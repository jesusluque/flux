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
#include <cstring>
#include <cstdlib>
#include <cstdio>
#include <thread>

using namespace filament;
using namespace filament::gltfio;
using filament::math::mat4f;
using filament::math::float3;
using utils::Entity;
using utils::EntityManager;

/* ─── FilamentScene struct ───────────────────────────────────────────────── */
struct FilamentScene {
    int width;
    int height;

    /* Color grading configuration (set at create time) */
    int  color_space_mode;   /* FILAMENT_CS_* constant */
    bool ycbcr_output;

    Engine*           engine;
    SwapChain*        swapChain;
    Renderer*         renderer;
    Scene*            scene;
    View*             view;
    Camera*           camera;
    Entity            cameraEntity;
    Skybox*           skybox;
    ColorGrading*     colorGrading;  /* NULL if post-processing disabled */

    /* gltfio */
    MaterialProvider* materials;
    AssetLoader*      assetLoader;
    ResourceLoader*   resourceLoader;
    FilamentAsset*    asset;

    /* Per-frame video texture — rebuilt each frame */
    Texture*          videoTexture;

    /* Scratch readback buffer */
    uint8_t*          readbackBuf;
};

/* ─── Public C API ───────────────────────────────────────────────────────── */

FilamentScene* filament_scene_create(int width, int height,
                                     const uint8_t* glb_data,
                                     size_t         glb_size,
                                     int            color_space_mode,
                                     int            ycbcr_output)
{
    FilamentScene* s = (FilamentScene*)calloc(1, sizeof(FilamentScene));
    if (!s) return nullptr;
    s->width            = width;
    s->height           = height;
    s->color_space_mode = color_space_mode;
    s->ycbcr_output     = (ycbcr_output != 0);

    /* Engine — headless OpenGL */
    s->engine = Engine::create(Engine::Backend::OPENGL);
    if (!s->engine) { free(s); return nullptr; }

    /* Offscreen SwapChain */
    s->swapChain = s->engine->createSwapChain(
        (uint32_t)width, (uint32_t)height,
        SwapChain::CONFIG_READABLE);

    s->renderer = s->engine->createRenderer();
    s->scene    = s->engine->createScene();
    s->view     = s->engine->createView();

    /* Camera */
    s->cameraEntity = EntityManager::get().create();
    s->camera       = s->engine->createCamera(s->cameraEntity);
    s->camera->setProjection(45.0, (double)width / (double)height, 0.1, 100.0);
    s->camera->lookAt({0.0f, 0.0f, 4.0f}, {0.0f, 0.0f, 0.0f}, {0.0f, 1.0f, 0.0f});

    /* Skybox — dark background */
    s->skybox = Skybox::Builder()
        .color({0.05f, 0.05f, 0.05f, 1.0f})
        .build(*s->engine);
    s->scene->setSkybox(s->skybox);

    s->view->setScene(s->scene);
    s->view->setCamera(s->camera);
    s->view->setViewport({0, 0, (uint32_t)width, (uint32_t)height});

    /* ── ColorGrading post-processing ─────────────────────────────────── */
    /* Build a ColorGrading LUT for the requested output color space.
     * Post-processing must be enabled for ColorGrading to take effect. */
    {
        using namespace filament::color;

        /* Map mode constant → color::ColorSpace DSL expression */
        color::ColorSpace cs = Rec709 - sRGB - D65;  /* default: sRGB */
        switch (color_space_mode) {
        case FILAMENT_CS_BT709:          cs = Rec709  - BT709  - D65; break;
        case FILAMENT_CS_REC709_LINEAR:  cs = Rec709  - Linear - D65; break;
        case FILAMENT_CS_REC2020_LINEAR: cs = Rec2020 - Linear - D65; break;
        case FILAMENT_CS_REC2020_PQ:     cs = Rec2020 - PQ     - D65; break;
        case FILAMENT_CS_REC2020_HLG:    cs = Rec2020 - HLG    - D65; break;
        default: break; /* FILAMENT_CS_SRGB — already set above */
        }

        s->colorGrading = ColorGrading::Builder()
            .outputColorSpace(cs)
            .ycbcrOutput(s->ycbcr_output)
            .build(*s->engine);

        s->view->setColorGrading(s->colorGrading);
        s->view->setPostProcessingEnabled(true);
    }

    /* ── gltfio setup ──────────────────────────────────────────────────── */

    /* UbershaderProvider uses the pre-built material archive bundled in the
     * Filament distribution — no matc compilation step required. */
    s->materials = createUbershaderProvider(
        s->engine,
        UBERARCHIVE_DEFAULT_DATA,
        UBERARCHIVE_DEFAULT_SIZE);

    utils::NameComponentManager* ncm =
        new utils::NameComponentManager(EntityManager::get());

    s->assetLoader = AssetLoader::create({s->engine, s->materials, ncm});

    /* Parse the GLB — creates Filament entities but does NOT upload GPU data */
    s->asset = s->assetLoader->createAsset(glb_data, (uint32_t)glb_size);
    if (!s->asset) {
        fprintf(stderr, "fluxvideotex: gltfio failed to parse GLB\n");
        /* cleanup minimal */
        AssetLoader::destroy(&s->assetLoader);
        s->materials->destroyMaterials();
        delete s->materials;
        delete ncm;
        Engine::destroy(&s->engine);
        free(s);
        return nullptr;
    }

    /* ResourceLoader uploads vertex/index buffers and handles embedded images.
     * Our cube.glb has a flux:// URI image — ResourceLoader won't find data
     * for it (no addResourceData call for that URI), so it will leave that
     * texture slot empty.  We fill it manually each frame. */
    ResourceConfiguration rcfg{};
    rcfg.engine = s->engine;
    rcfg.gltfPath = nullptr;
    rcfg.normalizeSkinningWeights = false;
    s->resourceLoader = new ResourceLoader(rcfg);
    s->resourceLoader->loadResources(s->asset);

    /* Add all renderable entities to the scene */
    s->asset->releaseSourceData();
    size_t count = s->asset->getRenderableEntityCount();
    const Entity* entities = s->asset->getRenderableEntities();
    for (size_t i = 0; i < count; ++i) {
        s->scene->addEntity(entities[i]);
    }

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

    /* Bind placeholder to all material instances now */
    TextureSampler sampler(
        TextureSampler::MinFilter::LINEAR,
        TextureSampler::MagFilter::LINEAR);
    FilamentInstance* inst = s->asset->getInstance();
    if (inst) {
        size_t mcount = inst->getMaterialInstanceCount();
        MaterialInstance* const* mis = inst->getMaterialInstances();
        for (size_t i = 0; i < mcount; ++i) {
            mis[i]->setParameter("baseColorMap", s->videoTexture, sampler);
        }
    }

    s->readbackBuf = (uint8_t*)malloc((size_t)width * height * 4);
    return s;
}

void filament_scene_destroy(FilamentScene* s)
{
    if (!s) return;

    /* Remove renderable entities from scene first */
    size_t count = s->asset->getRenderableEntityCount();
    const Entity* entities = s->asset->getRenderableEntities();
    for (size_t i = 0; i < count; ++i) {
        s->scene->remove(entities[i]);
    }

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
    free(s);
}

void filament_scene_render(FilamentScene* s,
                           const uint8_t* in_rgba, int in_w, int in_h,
                           double elapsed_s,
                           double period_x, double period_y, double period_z,
                           uint8_t* out_rgba)
{
    /* ── Upload video frame as new Filament texture ─────────────────────── */
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

    /* Bind to all material instances — this is the core flux:// resolution
     * step: replace the GLB image that had uri "flux://channel/0" with the
     * live decoded video frame, matching §10.10.5 precedence rules. */
    TextureSampler sampler(
        TextureSampler::MinFilter::LINEAR,
        TextureSampler::MagFilter::LINEAR);
    FilamentInstance* inst = s->asset->getInstance();
    if (inst) {
        size_t mcount = inst->getMaterialInstanceCount();
        MaterialInstance* const* mis = inst->getMaterialInstances();
        for (size_t i = 0; i < mcount; ++i) {
            mis[i]->setParameter("baseColorMap", s->videoTexture, sampler);
        }
    }

    /* ── Animate cube rotation ──────────────────────────────────────────── */
    float ax = (float)(2.0 * M_PI * elapsed_s / period_x);
    float ay = (float)(2.0 * M_PI * elapsed_s / period_y);
    float az = (float)(2.0 * M_PI * elapsed_s / period_z);

    mat4f rot = mat4f::rotation(ax, float3{1, 0, 0})
              * mat4f::rotation(ay, float3{0, 1, 0})
              * mat4f::rotation(az, float3{0, 0, 1});

    auto& tcm = s->engine->getTransformManager();
    /* Apply rotation to the root entity of the GLB asset */
    Entity root = s->asset->getRoot();
    tcm.setTransform(tcm.getInstance(root), rot);

    /* ── Render + readback inside a single frame ───────────────────────── */
    /* readPixels() must be called within a frame (after render, before endFrame).
     * readPixels is ASYNCHRONOUS — the callback fires on the main thread once
     * the GPU DMA into readbackBuf is complete (typically several frames later).
     * We must pump engine->execute() until the callback fires before we can
     * memcpy the data out. */
    std::atomic<bool> readback_done{false};

    if (s->renderer->beginFrame(s->swapChain)) {
        s->renderer->render(s->view);

        backend::PixelBufferDescriptor readPbd(
            s->readbackBuf,
            (size_t)s->width * s->height * 4,
            backend::PixelDataFormat::RGBA,
            backend::PixelDataType::UBYTE,
            [](void* /*buf*/, size_t /*sz*/, void* user) {
                static_cast<std::atomic<bool>*>(user)->store(true,
                    std::memory_order_release);
            },
            &readback_done);

        s->renderer->readPixels(0, 0, (uint32_t)s->width, (uint32_t)s->height,
                                std::move(readPbd));

        s->renderer->endFrame();
    } else {
        /* Frame was skipped for pacing — no new readback this call.
         * Output whatever is already in readbackBuf (previous frame or grey). */
        memcpy(out_rgba, s->readbackBuf, (size_t)s->width * s->height * 4);
        return;
    }

    /* Flush commands to the backend, then spin-pump the main-thread callback
     * queue until the GPU readback DMA signals completion.
     * NOTE: engine->execute() is a no-op on macOS (OpenGL backend runs its own
     * thread).  pumpMessageQueues() drains the user-callback queue on the
     * calling thread — this is what makes the readPixels callback fire. */
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
