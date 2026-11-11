# Obscure Explorer

A read-only asset explorer (FModel style) for **Obscure 1**, **Obscure 2** and **Final Exam**: browse the `.hvp`
archives and loose game files, search, preview, and export. Nothing is re-encoded or repacked.

Português: [README.pt.md](README.pt.md)

## What it opens

| Kind | Files | What you get |
|------|-------|--------------|
| Archives | `.hvp` (a whole game folder, or one archive) | every archive shows as a folder; the tree and the tile / list view work like a file explorer |
| Textures | `.dic` (Obscure 2 PC), `.dip` (Obscure 1 PC), `.xbr` (Obscure 1 Xbox), `.dic` and `.hvt` (Obscure 2 Wii), `.dic` (PSP), `.dic` and `.hvi` (PS2), `.hvt` (Final Exam PC / PS3 / Xbox 360), `.bmp` `.tga` `.png` `.jpg` `.ico` | thumbnails, full-window viewer, export as PNG. Packs open like folders |
| Audio | `.wav` `.mp3` `.ogg` `.flac`, `.xwma` (Final Exam), `.at3` (PSP), `.wsi` (Wii) | player bar with seek, volume, previous / next. `.xwma` and `.at3` need ffmpeg |
| Video | `.bik` (Bink), `.ogv`, `.pmf` (PSP) | full-window player with seek and previous / next. Needs ffmpeg |
| 3D models | `.zwo` (Obscure 1 and 2) | 3D viewer: textured / solid / wireframe, mesh list, texture status. Skinned meshes show their rest pose |
| Text | `.txt` `.ini` `.xml` `.csv` `.srt` `.lua` `.cre` `.sub` ... | text window with search (UTF-8, UTF-16 and Windows-1252). Compiled Lua shows its strings |
| Text (converted) | `.lng` (all three games) | readable text; export as `.lng`, `.txt` or both |
| Text (converted) | `.xmc` (Obscure 2) | XML; export as `.xmc`, `.xml` or both |
| Flash | `.swf` (Final Exam) | summary window and "Play in Ruffle" |
| Animation data | `.ani` (Final Exam), `*anim*.zwo` | orange **ANIM** tag; no preview yet |

Not supported yet: skeletons and animations of `.zwo`, PS2 `.zwo`,
`.hoe` files.

## Getting started

1. Build and run: `cargo run -p obscure-explorer --release` (a recent stable Rust; a release build opens without a console window).
2. **Open folder...** and pick the game folder: every `.hvp` inside is opened (each one shows up as a folder named after
   the file) together with the loose files (`data` folder, `.exe`, ...). **Open HVP...** opens a single archive.
3. The game and the platform are detected automatically (disc folders such as `PSP_GAME`, executables like `default.xbe`,
   `main.dol` or `SLES_123.45`, Steam / GOG launcher files, `.pmf` / `.at3` / `.xbr` files, or the layout of the first `.dic`).
   Change the **Platform** by hand if it guessed wrong: PC has Steam, GOG and Retail entries.
4. Obscure 2: copy the `hashes` folder of [obscure-hvp](https://github.com/YouKnow-sys/obscure-hvp) next to the program,
   otherwise the file names inside the archives are wrong.
5. The **Open folder / Open HVP** dialogs and the **Export** dialog each remember their own folder (saved between runs).
   Click things to open them. Right-click to export. Ctrl+click, Shift+click or the checkbox on a tile select several
   items; **Export...** writes the raw files (keeping the folder structure) and / or converted copies (PNG, XML, TXT).

If something crashes, the program writes `obscure_explorer_crash.log` next to the executable.

## External tools

These are not bundled; each pop-up offers to set them up the first time they are needed.

- **ffmpeg** decodes `.bik`, `.ogv`, `.pmf`, `.xwma` and `.at3`. Windows: downloaded automatically (gyan.dev build). Linux: a static build
  is downloaded, or the pop-up shows the install command of your distribution. You can also point to an existing ffmpeg.
- **Ruffle** (free Flash emulator) plays `.swf` files in its own window. Downloaded from its GitHub releases (Windows and
  Linux) or located by you.
- **hashes.json** gives names to the tags and attributes of `.xmc` files (put it next to the program, or use
  "Locate hashes.json..." in the window). Without it they show as `tag_XXXXXXXX`.

Downloaded tools and the files that remember their location are kept next to the program, or in
`~/.local/share/obscure-explorer` when that folder is read-only.

## 3D models

Click a `.zwo` to open the viewer: drag to orbit, right-drag to pan, wheel to zoom, **F** to frame, **Esc** to close.
Textures are looked up in the `.dic` / `.dip` next to the model, then in the parent folder (Obscure 2 characters:
`boy1/skins/boy1_pc.zwo` uses `boy1/boy1_pc.dic`), then in the shared packs at the root of each HVP (weapons and items
use `datapack/texpc_pc.dic`).

## Keyboard

The panel under the mouse pointer has the keyboard focus.

- Items: arrows or W/A/S/D move the cursor, Enter opens, Ctrl+A selects all, Esc clears the selection, Backspace goes up.
- Folder tree: Up/Down move, Right expands or opens a pack, Left collapses or goes to the parent.
- Audio / video: Space plays or pauses; in the video player Left / Right seek 5 s, N / P change video.
- Image viewer: Left / Right (or A / D) change image, wheel zooms, drag pans, double-click toggles fit / 100%.
- Text windows: Ctrl+F searches, Enter / Shift+Enter (or F3) jump between matches.

## Project layout

| Crate | Purpose |
|-------|---------|
| `crates/core` | `Archive` trait, folder and `.hvp` archives, the file tree, game / platform context |
| `crates/textures` | texture decoders (`.dic`, `.dip`, `.hvt`, BC1/2/3, plain images) |
| `crates/models` | `.zwo` reader (materials, meshes, instances; entities it does not need are skipped) |
| `crates/xmc` | `.xmc` to XML, plus a small JSON reader |
| `crates/lng` | `.lng` to text (Obscure 1, Obscure 2, Final Exam) |
| `crates/swf` | `.swf` header and tag summary |
| `crates/explorer` | the egui / eframe 0.29 application |

## Credits and licenses

- Archives: [obscure-hvp](https://github.com/YouKnow-sys/obscure-hvp) by YouKnow-sys.
- Textures: the formats come from [ObsCure-Texture-Editor](https://github.com/HeitorSpectre/ObsCure-Texture-Editor)
  by HeitorSpectre (MIT; with Evil Trainer and marcos_haf), and from
  [obscure-texture-tool](https://github.com/Kapie77/obscure-texture-tool) by Kapie77 (GPL-3.0).
- `.xmc`: port of [Obscure-XMC-Converter](https://github.com/Kapie77/Obscure-XMC-Converter) by Kapie77 (GPL-3.0-or-later).
- `.lng`: port of the extraction code of [ObsCure-Text-Editor](https://github.com/HeitorSpectre/ObsCure-Text-Editor) by HeitorSpectre.
- `.zwo`: file layout from [zwoBlender](https://github.com/Al-Hydra/zwoBlender) by Al-Hydra, checked against real game files.
- Flash: [Ruffle](https://ruffle.rs). Video / audio conversion: [FFmpeg](https://ffmpeg.org).

The project includes code ported from GPL-3.0 programs by its own author, so it is meant to be distributed under
GPL-3.0-or-later. Check the licenses of the other sources listed above before redistributing builds.
