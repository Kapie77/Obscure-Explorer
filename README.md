# Obscure Explorer

Read-only asset explorer (FModel style) for Obscure 1, Obscure 2 and Final Exam:
browse, search, preview textures and extract files. No re-encoding / repacking.

![image1](https://i.ibb.co/CpzByKc3/obscureexplorer3.png)
![image2](https://i.ibb.co/SDhxGNZN/obscureexplorer2.png)
![image3](https://i.ibb.co/zv6BMY9/obscure4.png)
![image3](https://i.ibb.co/27BdCLkh/obscure5.png)

## Usage

1. `cargo run -p obscure-explorer --release`
2. "Open folder..." and pick the game folder: every `.hvp` inside is opened (each shows up as a folder named after the file)
   together with all loose files (e.g. the `data` folder with `.ogg` audio, `.exe`, `.dll`, ...). "Open HVP..." opens a
   single archive. The game is detected automatically; pick the Platform when needed (PC has Steam, GOG and Retail entries; Steam and GOG installs are recognised by their launcher files).
3. Obscure 2: copy the `hashes` folder from the obscure-hvp repository next to the executable
   (or into the directory you run `cargo run` from), otherwise file names will be wrong.
4. The folder tree (left, with its own filter box) lists folders and files; the center view shows the current folder as
   tiles or a list. Search all files with the center search box.
5. Clicking opens things in the middle, FModel style (unsupported files do nothing):
   - folders open; texture packs (`.dic`, `.dip`) open like folders with all their images (tinted purple, tagged PACK);
   - images (`.bmp`, `.tga`, `.png`, `.jpg`, the game's `.hvt`) and the images inside a pack open in a full-window viewer
     (wheel = zoom, drag = pan, double-click = fit / 100%, Left / Right or the Previous / Next buttons = other images of the same folder or pack, Esc = close);
   - audio (`.wav`, `.mp3`, `.ogg`, `.flac`) plays in the player bar at the bottom (previous / next, seek, volume, auto-next);
     the play badge on the thumbnail toggles play / pause;
   - video (`.bik`, `.ogv`) opens a full-window player (seek bar, previous / next video, Left / Right = -5 s / +5 s, N / P, Space, Esc). It needs ffmpeg:
     a pop-up offers to download it automatically (Windows) or to locate it;
   - text files (`.txt`, `.ini`, `.xml`, `.lng`, `.cre`, `.sub`, ...) open in a text window;
   - 3D models (`.zwo`, Obscure 1 and 2) open in a 3D viewer (drag = orbit, right-drag = pan, wheel = zoom, F = frame):
     textured / solid / wireframe, per-mesh visibility and a "Z axis is up" switch. Textures come from the `.dic` / `.dip`
     with the same name in the same folder. Skeletons and animations are not read yet (skinned meshes show their rest pose).
     Animation files (`anims_pc.zwo`, `crabanim.zwo`, ... anything named `*anim*.zwo`) carry an orange ANIM tag: they hold no geometry.
6. Selecting: tick the checkbox that appears on a tile, or Ctrl+click / Shift+click, or "Select all" (Ctrl+A).
   "Export..." (toolbar) exports the selection; right-click any item, in the center or in the folder tree, to export it
   (folders are exported recursively). The export window can write the raw files (keeping the folder structure) and/or
   textures as PNG (images inside a pack become separate PNG files), into a folder you choose.
7. Theme: Dark / Light (top right).

## Keyboard

The panel under the mouse pointer has the keyboard focus (clicking also sets it).
- Items: Arrow keys or W/A/S/D move the cursor (in the grid, up/down jump a row); Enter opens it like a click. Ctrl+A selects all, Esc clears the selection.
- Folder tree: Up/Down (W/S) move between folders; Right (D) expands or opens a pack; Left (A) collapses or goes to the parent.
- Backspace: up one level. Space: play / pause.

## Layout

- `crates/core`: `Archive` trait, `DirArchive` (extracted folder), `HvpArchive` (reads .hvp directly), `DirNode` (tree), `Context` (game + platform).
- `crates/models`: `.zwo` model reader (ported from zwoBlender by Al-Hydra).
- `crates/textures`: `TextureFormat` trait, pixel decoders (`pixel.rs`), BC1/2/3 (`bc.rs`), PC .dic/.dip parsers (`dic.rs`).
- `crates/explorer`: egui/eframe 0.29 GUI.

## Next steps

1. Wii / PS2 / PSP / Xbox texture formats of Obscure 1 and 2.
2. PS2/PSP (swizzle + palette), Wii (I8, IA8, RGB5A3, RGBA8, C4/C8, CMPR), Xbox (Morton), X360/PS3 (tiling/swizzle).
3. Folder/selection export.

## Credits

Texture formats come from the reverse-engineering work in [ObsCure-Texture-Editor](https://github.com/HeitorSpectre/ObsCure-Texture-Editor) (MIT; HeitorSpectre, Evil Trainer, marcos_haf)

The `.hvp` format comes from [obscure-hvp](https://github.com/YouKnow-sys/obscure-hvp) (MIT; YouKnow-sys).

The `.zwo` format comes from [zwoBlender](https://github.com/Al-Hydra/zwoBlender) (Al-Hydra); check its license before redistributing.
