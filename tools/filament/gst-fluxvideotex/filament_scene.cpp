/*
 * filament_scene.cpp — Filament offscreen renderer for fluxvideotex (poc003)
 *
 * Loads a GLB cube (embedded at build time via xxd) using gltfio.
 * The cube's baseColorTexture is tagged  flux://channel/0  per FLUX Protocol
 * Spec v0.6.3 §10.10, and is replaced each frame with the incoming RGBA video
 * buffer — exactly what the real fluxvideotex element does for live video
 * texture binding (§10.8 / §10.10.5).
 *
 * Architecture:
 *   - Engine::Backend::OPENGL (headless, no platform window)
 *   - Offscreen SwapChain (CONFIG_READABLE) → renderer->readPixels() works
 *   - gltfio UbershaderProvider: pre-built PBR materials, no matc step
 *   - Per-frame: upload GStreamer RGBA → filament::Texture → setParameter on
 *     every material instance in the asset → render → readPixels → vflip
 *
 * Thread model:
 *   Filament requires that Engine::create() and Engine::destroy() are called
 *   from the SAME thread.  GObject finalization (where filament_scene_destroy
 *   is called) runs on the GLib main thread, which differs from the GStreamer
 *   streaming thread that lazily calls filament_scene_create → Engine::create.
 *
 *   To satisfy this constraint we run ALL Filament operations on a single
 *   dedicated owner thread (FilamentScene::owner_thread).  The public C API
 *   functions post work items to that thread via a mutex+condvar and block
 *   until the work completes.  This guarantees Engine::create and
 *   Engine::destroy always execute on the same thread regardless of which
 *   thread the GStreamer element calls us from.
 */

#include "filament_scene.h"

#include <filament/Engine.h>
#include <filament/Camera.h>
#include <filament/ColorGrading.h>
#include <filament/ColorSpace.h>
#include <filament/Material.h>
#include <filament/MaterialInstance.h>
#include <filament/RenderableManager.h>
#include <filament/Renderer.h>
#include <filament/Scene.h>
#include <filament/Skybox.h>
#include <filament/SwapChain.h>
#include <filament/Texture.h>
#include <filament/TextureSampler.h>
#include <filament/TransformManager.h>
#include <filament/View.h>
#include <filament/Viewport.h>

#include <backend/PixelBufferDescriptor.h>

#include <gltfio/AssetLoader.h>
#include <gltfio/FilamentAsset.h>
#include <gltfio/FilamentInstance.h>
#include <gltfio/MaterialProvider.h>
#include <gltfio/ResourceLoader.h>
#include <gltfio/TextureProvider.h>
#include <gltfio/materials/uberarchive.h>

#include <utils/EntityManager.h>
#include <utils/NameComponentManager.h>

#include <math/mat4.h>
#include <math/vec3.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstring>
#include <cstdlib>
#include <cstdio>
#include <functional>
#include <mutex>
#include <thread>

using namespace filament;
using namespace filament::gltfio;
using filament::math::mat4f;
using filament::math::float3;
using utils::Entity;
using utils::EntityManager;

/* ─── Work-item queue (single producer, single consumer) ────────────────── */

struct WorkItem {
    std::function<void()> fn;
    bool                  done = false;
};

/* ─── FilamentScene struct ───────────────────────────────────────────────── */
struct FilamentScene {
    int width;
    int height;

    /* Color grading configuration (set at create time) */
    int  color_space_mode;   /* FILAMENT_CS_* constant */
    bool ycbcr_output;

    /* Owner thread — ALL Filament calls happen here */
    std::thread             owner_thread;
    std::mutex              mtx;
    std::condition_variable cv;
    WorkItem*               pending = nullptr;   /* item posted by caller */
    bool                    quit    = false;      /* signals thread to exit */

    /* Filament objects — only touched from owner_thread */
    Engine*           engine       = nullptr;
    SwapChain*        swapChain    = nullptr;
    Renderer*         renderer     = nullptr;
    Scene*            scene        = nullptr;
    View*             view         = nullptr;
    Camera*           camera       = nullptr;
    Entity            cameraEntity = {};
    Skybox*           skybox       = nullptr;
    ColorGrading*     colorGrading = nullptr;

    /* gltfio */
    MaterialProvider* materials      = nullptr;
    AssetLoader*      assetLoader    = nullptr;
    ResourceLoader*   resourceLoader = nullptr;
    FilamentAsset*    asset          = nullptr;

    /* Per-frame video texture — rebuilt each frame */
    Texture*  videoTexture = nullptr;

    /* Scratch readback buffer */
    uint8_t*  readbackBuf  = nullptr;
};

/* ─── Owner-thread loop ──────────────────────────────────────────────────── */

static void owner_thread_loop(FilamentScene* s)
{
    for (;;) {
        WorkItem* item = nullptr;
        {
            std::unique_lock<std::mutex> lk(s->mtx);
            s->cv.wait(lk, [s]{ return s->pending != nullptr || s->quit; });
            if (s->quit && s->pending == nullptr)
                break;
            item = s->pending;
            s->pending = nullptr;
        }
        if (item) {
            item->fn();
            {
                std::unique_lock<std::mutex> lk(s->mtx);
                item->done = true;
            }
            s->cv.notify_all();
        }
    }
}

/* Post a work item to the owner thread and block until it finishes. */
static void run_on_owner(FilamentScene* s, std::function<void()> fn)
{
    WorkItem item;
    item.fn   = std::move(fn);
    item.done = false;
    {
        std::unique_lock<std::mutex> lk(s->mtx);
        s->pending = &item;
    }
    s->cv.notify_one();
    {
        std::unique_lock<std::mutex> lk(s->mtx);
        s->cv.wait(lk, [&item]{ return item.done; });
    }
}

/* ─── Internal Filament init (runs on owner thread) ─────────────────────── */

static bool filament_init(FilamentScene* s,
                          const uint8_t* glb_data, size_t glb_size)
{
    /* Engine — headless OpenGL */
    s->engine = Engine::create(Engine::Backend::OPENGL);
    if (!s->engine) return false;

    /* Offscreen SwapChain */
    s->swapChain = s->engine->createSwapChain(
        (uint32_t)s->width, (uint32_t)s->height,
        SwapChain::CONFIG_READABLE);

    s->renderer = s->engine->createRenderer();
    s->scene    = s->engine->createScene();
    s->view     = s->engine->createView();

    /* Camera */
    s->cameraEntity = EntityManager::get().create();
    s->camera       = s->engine->createCamera(s->cameraEntity);
    s->camera->setProjection(45.0,
        (double)s->width / (double)s->height, 0.1, 100.0);
    s->camera->lookAt({0.0f, 0.0f, 4.0f},
                      {0.0f, 0.0f, 0.0f},
                      {0.0f, 1.0f, 0.0f});

    /* Skybox — dark background */
    s->skybox = Skybox::Builder()
        .color({0.05f, 0.05f, 0.05f, 1.0f})
        .build(*s->engine);
    s->scene->setSkybox(s->skybox);

    s->view->setScene(s->scene);
    s->view->setCamera(s->camera);
    s->view->setViewport({0, 0, (uint32_t)s->width, (uint32_t)s->height});

    /* ── ColorGrading post-processing ───────────────────────────────────── */
    {
        using namespace filament::color;

        color::ColorSpace cs = Rec709 - sRGB - D65;  /* default */
        switch (s->color_space_mode) {
        case FILAMENT_CS_BT709:          cs = Rec709  - BT709  - D65; break;
        case FILAMENT_CS_REC709_LINEAR:  cs = Rec709  - Linear - D65; break;
        case FILAMENT_CS_REC2020_LINEAR: cs = Rec2020 - Linear - D65; break;
        case FILAMENT_CS_REC2020_PQ:     cs = Rec2020 - PQ     - D65; break;
        case FILAMENT_CS_REC2020_HLG:    cs = Rec2020 - HLG    - D65; break;
        default: break;
        }

        s->colorGrading = ColorGrading::Builder()
            .outputColorSpace(cs)
            .ycbcrOutput(s->ycbcr_output)
            .build(*s->engine);

        s->view->setColorGrading(s->colorGrading);
        s->view->setPostProcessingEnabled(true);
    }

    /* ── gltfio setup ───────────────────────────────────────────────────── */
    s->materials = createUbershaderProvider(
        s->engine,
        UBERARCHIVE_DEFAULT_DATA,
        UBERARCHIVE_DEFAULT_SIZE);

    utils::NameComponentManager* ncm =
        new utils::NameComponentManager(EntityManager::get());

    s->assetLoader = AssetLoader::create({s->engine, s->materials, ncm});

    s->asset = s->assetLoader->createAsset(glb_data, (uint32_t)glb_size);
    if (!s->asset) {
        fprintf(stderr, "fluxvideotex: gltfio failed to parse GLB\n");
        AssetLoader::destroy(&s->assetLoader);
        s->materials->destroyMaterials();
        delete s->materials;
        delete ncm;
        Engine::destroy(&s->engine);
        return false;
    }

    ResourceConfiguration rcfg{};
    rcfg.engine = s->engine;
    rcfg.gltfPath = nullptr;
    rcfg.normalizeSkinningWeights = false;
    s->resourceLoader = new ResourceLoader(rcfg);
    s->resourceLoader->loadResources(s->asset);

    s->asset->releaseSourceData();
    size_t count = s->asset->getRenderableEntityCount();
    const Entity* entities = s->asset->getRenderableEntities();
    for (size_t i = 0; i < count; ++i)
        s->scene->addEntity(entities[i]);

    /* Placeholder 1×1 grey video texture */
    s->videoTexture = Texture::Builder()
        .width(1).height(1).levels(1)
        .sampler(Texture::Sampler::SAMPLER_2D)
        .format(Texture::InternalFormat::RGBA8)
        .build(*s->engine);
    static const uint8_t grey[4] = {128, 128, 128, 255};
    Texture::PixelBufferDescriptor pbd(
        grey, 4,
        Texture::Format::RGBA, Texture::Type::UBYTE, nullptr);
    s->videoTexture->setImage(*s->engine, 0, std::move(pbd));

    TextureSampler sampler(
        TextureSampler::MinFilter::LINEAR,
        TextureSampler::MagFilter::LINEAR);
    FilamentInstance* inst = s->asset->getInstance();
    if (inst) {
        size_t mcount = inst->getMaterialInstanceCount();
        MaterialInstance* const* mis = inst->getMaterialInstances();
        for (size_t i = 0; i < mcount; ++i)
            mis[i]->setParameter("baseColorMap", s->videoTexture, sampler);
    }

    s->readbackBuf = (uint8_t*)malloc((size_t)s->width * s->height * 4);
    return true;
}

/* ─── Internal Filament teardown (runs on owner thread) ──────────────────── */

static void filament_teardown(FilamentScene* s)
{
    if (!s->engine) return;

    size_t count = s->asset->getRenderableEntityCount();
    const Entity* entities = s->asset->getRenderableEntities();
    for (size_t i = 0; i < count; ++i)
        s->scene->remove(entities[i]);

    s->assetLoader->destroyAsset(s->asset);
    delete s->resourceLoader;
    AssetLoader::destroy(&s->assetLoader);
    s->materials->destroyMaterials();
    delete s->materials;

    s->engine->destroy(s->videoTexture);
    s->engine->destroy(s->skybox);
    if (s->colorGrading)
        s->engine->destroy(s->colorGrading);
    s->engine->destroyCameraComponent(s->cameraEntity);
    EntityManager::get().destroy(s->cameraEntity);

    s->engine->destroy(s->view);
    s->engine->destroy(s->scene);
    s->engine->destroy(s->renderer);
    s->engine->destroy(s->swapChain);
    Engine::destroy(&s->engine);

    free(s->readbackBuf);
    s->readbackBuf = nullptr;
}

/* ─── Internal render (runs on owner thread) ─────────────────────────────── */

static void filament_render_frame(FilamentScene* s,
                                  const uint8_t* in_rgba, int in_w, int in_h,
                                  double elapsed_s,
                                  double period_x, double period_y,
                                  double period_z,
                                  uint8_t* out_rgba)
{
    /* Upload video frame as new Filament texture */
    s->engine->destroy(s->videoTexture);

    size_t nbytes = (size_t)in_w * in_h * 4;
    uint8_t* copy = (uint8_t*)malloc(nbytes);
    memcpy(copy, in_rgba, nbytes);

    s->videoTexture = Texture::Builder()
        .width((uint32_t)in_w).height((uint32_t)in_h).levels(1)
        .sampler(Texture::Sampler::SAMPLER_2D)
        .format(Texture::InternalFormat::RGBA8)
        .build(*s->engine);

    Texture::PixelBufferDescriptor pbd(
        copy, nbytes,
        Texture::Format::RGBA, Texture::Type::UBYTE,
        [](void* buf, size_t, void*) { free(buf); },
        nullptr);
    s->videoTexture->setImage(*s->engine, 0, std::move(pbd));

    TextureSampler sampler(
        TextureSampler::MinFilter::LINEAR,
        TextureSampler::MagFilter::LINEAR);
    FilamentInstance* inst = s->asset->getInstance();
    if (inst) {
        size_t mcount = inst->getMaterialInstanceCount();
        MaterialInstance* const* mis = inst->getMaterialInstances();
        for (size_t i = 0; i < mcount; ++i)
            mis[i]->setParameter("baseColorMap", s->videoTexture, sampler);
    }

    /* Animate cube rotation */
    float ax = (float)(2.0 * M_PI * elapsed_s / period_x);
    float ay = (float)(2.0 * M_PI * elapsed_s / period_y);
    float az = (float)(2.0 * M_PI * elapsed_s / period_z);

    mat4f rot = mat4f::rotation(ax, float3{1, 0, 0})
              * mat4f::rotation(ay, float3{0, 1, 0})
              * mat4f::rotation(az, float3{0, 0, 1});

    auto& tcm = s->engine->getTransformManager();
    Entity root = s->asset->getRoot();
    tcm.setTransform(tcm.getInstance(root), rot);

    /* Render + readback */
    std::atomic<bool> readback_done{false};

    if (s->renderer->beginFrame(s->swapChain)) {
        s->renderer->render(s->view);

        backend::PixelBufferDescriptor readPbd(
            s->readbackBuf,
            (size_t)s->width * s->height * 4,
            backend::PixelDataFormat::RGBA,
            backend::PixelDataType::UBYTE,
            [](void* /*buf*/, size_t /*sz*/, void* user) {
                static_cast<std::atomic<bool>*>(user)->store(
                    true, std::memory_order_release);
            },
            &readback_done);

        s->renderer->readPixels(0, 0,
            (uint32_t)s->width, (uint32_t)s->height,
            std::move(readPbd));

        s->renderer->endFrame();
    } else {
        memcpy(out_rgba, s->readbackBuf, (size_t)s->width * s->height * 4);
        return;
    }

    s->engine->flushAndWait();
    while (!readback_done.load(std::memory_order_acquire)) {
        s->engine->pumpMessageQueues();
        std::this_thread::sleep_for(std::chrono::microseconds(100));
    }

    /* Filament readPixels is bottom-up; flip to top-down for GStreamer */
    int stride = s->width * 4;
    for (int row = 0; row < s->height / 2; ++row) {
        uint8_t* top = s->readbackBuf + row * stride;
        uint8_t* bot = s->readbackBuf + (s->height - 1 - row) * stride;
        for (int col = 0; col < stride; ++col) {
            uint8_t tmp = top[col];
            top[col]    = bot[col];
            bot[col]    = tmp;
        }
    }

    memcpy(out_rgba, s->readbackBuf, (size_t)s->width * s->height * 4);
}

/* ─── Public C API ───────────────────────────────────────────────────────── */

FilamentScene* filament_scene_create(int width, int height,
                                     const uint8_t* glb_data,
                                     size_t         glb_size,
                                     int            color_space_mode,
                                     int            ycbcr_output)
{
    FilamentScene* s = new FilamentScene();
    s->width            = width;
    s->height           = height;
    s->color_space_mode = color_space_mode;
    s->ycbcr_output     = (ycbcr_output != 0);

    /* Start the owner thread */
    s->owner_thread = std::thread(owner_thread_loop, s);

    /* Run Filament init on the owner thread */
    bool ok = false;
    run_on_owner(s, [s, glb_data, glb_size, &ok]{
        ok = filament_init(s, glb_data, glb_size);
    });

    if (!ok) {
        /* Signal quit and join */
        {
            std::unique_lock<std::mutex> lk(s->mtx);
            s->quit = true;
        }
        s->cv.notify_one();
        s->owner_thread.join();
        delete s;
        return nullptr;
    }

    return s;
}

void filament_scene_destroy(FilamentScene* s)
{
    if (!s) return;

    /* Run teardown on the owner thread, then stop it */
    run_on_owner(s, [s]{ filament_teardown(s); });

    {
        std::unique_lock<std::mutex> lk(s->mtx);
        s->quit = true;
    }
    s->cv.notify_one();
    s->owner_thread.join();

    delete s;
}

void filament_scene_render(FilamentScene* s,
                           const uint8_t* in_rgba, int in_w, int in_h,
                           double elapsed_s,
                           double period_x, double period_y, double period_z,
                           uint8_t* out_rgba)
{
    run_on_owner(s, [=]{
        filament_render_frame(s, in_rgba, in_w, in_h,
                              elapsed_s, period_x, period_y, period_z,
                              out_rgba);
    });
}
