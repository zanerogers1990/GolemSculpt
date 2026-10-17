# Golem Sculpt: Renderer Upgrade Plan

The goal is to take the viewport from "good sculpting viewport" to a renderer that makes finished projects look like studio product shots. That means renders good enough for store listings, turntables and client previews, without slowing down sculpting.

This is a hand-off for Claude Code working in this repo. Read **Ground rules** first. Zane will be using the app while you build.

---

## Ground rules: Zane is sculpting while you build

Zane may have `D:\Sculptingapp2\target\release\sculpt.exe` open the whole time. Breaking or blocking his session is not acceptable.

1. **Work only in a worktree.** Create one under `.claude/worktrees/` (for example `renderer`) on its own branch. Never build, edit or check out anything in `D:\Sculptingapp2` itself.
2. **Never touch his build.** Don't write to `D:\Sculptingapp2\target`, don't run `Sculpt.bat`, and never kill or restart a running `sculpt.exe`. The worktree builds into its own `target` folder.
3. **Keep his PC usable.** Build with a job cap (`cargo build --release -j 8`) so compiles don't starve his session. Avoid running the full test suite on loop; run the tests you touched, and the full suite once per phase.
4. **Keep his settings safe.** Any time you launch the dev build, set `SCULPT_SETTINGS_DIR` to a folder inside the worktree (for example `.claude\worktrees\renderer\.dev-settings`). That keeps `%APPDATA%\Sculpt`, his brushes and prefs, untouched. Recovery folders are already per-run and safe.
5. **Use the bridge carefully.** The repo's `.mcp.json` points at `D:\Sculptingapp2\Sculpt-mcp.bat`, which launches his **main** build, so anything checked through it shows the old renderer. To look at the dev build, use a bridge config local to the worktree that runs `Sculpt-mcp.bat` from the worktree. Never change the main `.mcp.json`.
6. **Leave merging to Zane.** Commit on the branch and tell him when a phase is ready. He closes the app before merging.
7. **After a merge, rebuild the main checkout.** `Sculpt.bat` only builds when `sculpt.exe` is missing, so it would keep launching the old exe. Run `cargo build --release` in `D:\Sculptingapp2` once Zane has closed the app.

---

## Where things are now

| Area | File(s) | State |
|---|---|---|
| Frame setup, passes, scene cache | `src/render/mod.rs` | HDR (Rgba16F) forward pass, 4x MSAA, reverse depth. A `scene_key` skips redrawing unchanged frames. |
| Surface shading and finishes | `src/render/sculpt_pbr.wgsl`, `shader.wgsl` | Port of Unity's built-in BRDF (cheap normalised GGX, `pow(1-nv,4)` fresnel). Up to 5 directional lights. Finishes: PBR, lure plastic, metal, clay, wood, glow, matcap. |
| Painted surfaces | `src/render/painted.wgsl`, `paint.rs` | Shares `mesh_surface` / `light_sample` with the above, so shading changes must land in both. |
| Key shadow | `look.rs` (`SHADOW_SIZE = 512`), `key_shadow()` | One 512² map, key light only, 7x7 tent PCF. Cached while lights are frozen. |
| Environment | `src/render/env.rs`, `env.wgsl` | 128² cube, GGX-prefiltered mips, SH ambient from the HDRI. |
| AO / cavity | `post.wgsl` `fs_ssao`, `shader.wgsl` `cavity()` | 8-sample hemisphere SSAO with bilateral blur; screen-space cavity from the normals prepass. |
| Post | `post.rs`, `post.wgsl`, `dof.rs` | Bloom, vignette, Neutral/ACES tonemap, grading, depth of field. |
| Offscreen pictures | `capture.rs`, `render::Side`, `app/recording.rs`, `turntable.rs` | Custom-size targets for loops and timelapses. |
| Perf check | `src/bench_render.rs` | Use it before and after each phase. |

---

## Rules for every change

- **Keep the frame cache honest.** Any new setting that changes the picture goes into `scene_key`. Anything that draws over several frames (accumulation) must restart the moment the key changes.
- **Keep painted and bare surfaces identical.** Shading changes go through the shared functions so `painted.wgsl` picks them up. Test a painted object and a bare one side by side.
- **Leave the matcap path alone.** It's baked lighting and it's what people sculpt with. Nothing in this plan should change how matcaps look or how fast they draw.
- **Don't slow down sculpting.** While a stroke is running, the viewport must be no slower than today (check with `bench_render`). Expensive features run only at rest or in Render mode, or sit behind a quality setting.
- **Old files keep loading.** New `Look` fields need serde defaults so existing scenes and presets open unchanged.
- **Match the house style.** Code comments use the plain-English style already in the repo, and every phase comes with tests like the existing ones.

---

## Phase 1: correct light (small, big payoff)

**1.1 Proper specular response.** Replace the Unity mobile approximation in `physically_shade`:
- Split-sum image-based lighting with a DFG lookup table: a precomputed 2D texture, or Karis's analytic fit if a texture is a hassle.
- Multi-scattering energy compensation (Fdez-Agüera), so rough metals don't go dark.
- Proper Schlick fresnel (`pow5`) and a height-correlated Smith visibility term for direct lights.
- Specular occlusion derived from AO (Lagarde's approximation), so creases stop reflecting the sky.
- *Check:* a sphere sweep of metal and dielectric at roughness 0 to 1 holds brightness across the range. No finish preset should change character, only get more accurate. Compare before/after screenshots of every preset.

**1.2 Shadows.**
- Make `SHADOW_SIZE` a quality setting (512 / 2048 / 4096, default 2048) and fit the light's frustum tightly to the visible objects.
- Replace the fixed tent with PCSS: a blocker search, then a variable-width filter, so shadows sharpen at contact and soften with distance. Add a light-size setting per preset.
- *Check:* no acne or peter-panning on the default test shapes, and the shadow is still cached when lights are frozen.

**1.3 Reflections.** Raise the env cube to 256 (512 as a quality option). For roughness near 0, sample the HDRI directly so chrome and glossy plastic get a sharp reflection.

**1.4 Tone and exposure.**
- Add an exposure control in EV stops.
- Add Khronos PBR Neutral and AgX tonemappers next to the existing ones. Default new scenes to Khronos PBR Neutral, which keeps product colours accurate (important for lure colourways). Existing scenes keep the tonemap they saved.

## Phase 2: refine at rest and a Render mode (the big one)

**2.1 Progressive accumulation when idle.** When `scene_key` is unchanged for a frame, start accumulating instead of just blitting the cached frame:
- Per sample, add sub-pixel camera jitter (Halton), rotated SSAO/shadow noise, a fresh PCSS kernel rotation, and DOF lens samples.
- Add samples into an Rgba32F accumulation buffer and show the running average through the existing post chain.
- Stop at N samples (setting, default 64), then keep showing the result at no cost.
- Any change to the key resets it, so it must never be seen during a stroke or an orbit.
- *Check:* sculpting and orbiting feel identical to today, and stopping converges to a clean, aliasing-free picture in roughly 1 to 2 seconds.

**2.2 Render button.** A still render through the existing `capture` / `Side` path:
- Resolution multiplier 1x to 4x, sample count, transparent background option, PNG out.
- The turntable recorder gets a "samples per frame" option that uses the same accumulation.

**2.3 Ground plane (shadow catcher).**
- An optional floor under the lowest point of the visible objects that receives only the key shadow and contact AO, fading into the background.
- Optional glossy floor reflection: draw the scene mirrored, blurred by a roughness setting.
- It's a Look setting, part of the scene key, and off by default while sculpting.

## Phase 3: studio lighting and materials

**3.1 Rectangle area lights (softboxes).** LTC (linearly transformed cosines) rect lights for the preset rig, with a size per light. This is what produces those long, soft studio highlights on gloss. Keep directional lights as an option.

**3.2 Better AO and contact shadows.**
- Replace SSAO with GTAO (full or half resolution plus denoise), with bent normals feeding the specular occlusion from 1.1.
- Add screen-space contact shadows for all lights: a short depth-buffer march, so fine sculpt detail picks up crisp crease shadows the shadow map can't resolve.

**3.3 Translucent resin finish.** Generalise the lure plastic's back-face thickness trick into a "resin" finish: tint by thickness, light transmission, gloss. Add presets for clear, smoky and coloured resins so Zane can preview prints the way buyers will see them. Optionally add separable screen-space subsurface scattering for wax, clay and skin.

**3.4 Post extras (cheap).** Optional film grain, sharpening, and `.cube` LUT loading for colour grading.

## Phase 4 (optional, later): ray-traced Render mode

A path-traced or hardware-ray-traced (shadows/AO) mode used only for the Render button. wgpu's ray-query support is still experimental, so first check it's exposed on this machine's backend. The CPU BVH (`mesh/bvh.rs`) could be flattened to the GPU as a fallback. Only start this once Phases 1 to 3 are merged and Phase 2's accumulation has proven to be the bottleneck in quality.

---

## Order of work and hand-offs

1. Set up the worktree, a dev settings folder and a dev bridge config. Record `bench_render` numbers as the baseline.
2. Phase 1 (1.1 → 1.4), one commit per step with before/after screenshots. Tell Zane it's ready to try.
3. Phase 2 (2.1 → 2.3). Tell Zane it's ready.
4. Phase 3, in whatever order Zane picks.
5. For each hand-off, give Zane a short note: what changed, how to try it in the dev build, the bench numbers against the baseline, and anything that looks different in his existing scenes.
