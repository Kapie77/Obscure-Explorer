# Obscure XMC Converter
A CLI tool for converting .xmc to .xml — and .xml to .xmc — from Obscure 1/2
(PC, PS2, Wii and PSP) and Final Exam (PC and Xbox 360), which share the same
XML vocabulary in four different binary containers.

## Supported Versions

| Game                     | PC     | PS2    | Wii     | PSP    | Xbox 360 |
|--------------------------|:------:|:------:|:-------:|:------:|:--------:|
| Obscure 2                |   ✅   |   ✅   |   ✅   |   ✅  |          |
| Final Exam               |   ✅   |        |         |        |    ✅    |

# How to Compile
## Windows
Install Rust, and in that folder, run:

```powershell
cargo build --release
```

The executable will be located in:

`target\release\obscure-xmc-converter.exe`

## Linux
**Install Rust**

Open the terminal and run:
```bash
sudo apt update
sudo apt install build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the installer's instructions. Then, load the Rust environment:
```bash
source "$HOME/.cargo/env"
```

Check to see if it's installed:
```bash
rustc --version
cargo --version
```

**Compile the project**
```bash
cargo build --release
```

The executable will be located in:

`target/release/obscure-xmc-converter`

## Use via terminal
**Windows**
```bash
obscure-xmc-converter.exe extract filename.xmc
obscure-xmc-converter.exe build filename.xml
```

**Linux**
```bash
obscure-xmc-converter extract filename.xmc
obscure-xmc-converter build filename.xml
```

or

```bash
./obscure-xmc-converter extract filename.xmc
./obscure-xmc-converter build filename.xml
```

## Typical use

```powershell
# one file, names resolved from the shipped dictionary
obscure-xmc-converter.exe extract file.xmc --dict hashes.json

# a whole tree at once (PowerShell splats the array)
$files = (Get-ChildItem .\xmc -Recurse -Filter *.xmc).FullName
obscure-xmc-converter.exe extract @files --output .\xml --dict hashes.json

# rebuild them all; the format is read from the XML marker
$xml = (Get-ChildItem .\xml -Recurse -Filter *.xml).FullName
obscure-xmc-converter.exe build @xml --output .\xmc --dict hashes.json
```

Outputs are named after the input file stem, so when converting a whole tree
into a single output directory, keep the source names unique.

The format is picked automatically, so the same commands work for Obscure 1/2
and Final Exam files.

## Notes

- The Obscure binary format is little-endian on PC, PS2 and PSP and big-endian
  on Wii, with a 12-byte header, 16-byte node records, and 8-byte attributes.
- The format described does not represent XML text as node content; this conversion also preserves only elements and attributes.
- The XML-to-XMC conversion is a reconstruction of the format; it does not guarantee that the resulting bytes will be identical to those in the original file. Test on copies of the files and verify them in the game before replacing any files.

## Container formats

There are four layouts, all detected automatically:

| | Obscure 1/2 (PC/PS2/PSP) | Obscure 2 (Wii) | Final Exam (PC) | Final Exam (Xbox 360) |
|---|---|---|---|---|
| Byte order | little-endian | **big-endian** | little-endian | **big-endian** |
| Header | 12 bytes (`u32` node/attr/string counts) | same | 17 bytes (`u32` magic `0x66`, `u8` version `3`, three 24-bit counts) | 17 bytes, same fields with full 32-bit counts |
| Node | 16 bytes: `first_attr` is `u16`, then `u16` padding | 16 bytes: `first_attr` is a full `u32` | 20 bytes: adds `u16` padding and `u32` `0xFFFFFFFF` | 20 bytes: `first_attr` is a full `u32`, then `u32` `0xFFFFFFFF` |
| String pool | plain CP1252 | plain CP1252 | encrypted with a keystream derived from the three counts | same cipher as PC |

The PS2 and PSP releases of Obscure 2 use the same container as the PC release:
little-endian, 12-byte header, 16-byte nodes with a `u16` `first_attr` and `u16`
padding (always `0`), and a plain string pool. All 34 shipped PS2 files and all
37 shipped PSP files match this layout, so they need no separate format flag —
`auto` and `obscure2` both handle them.

`extract` sniffs the file: Final Exam is recognised by its magic and exact size
(either byte order), and a Wii file is a big-endian Obscure header whose counts
add up to the file size. `build` reads the `<!-- xmc-format: ... -->` comment
that `extract` writes (`wii`, `final-exam` or `xbox360`) and falls back to the
Obscure 1/2 layout for XML that has no such marker.

`--format auto` is the default; `--format obscure2` (PC/PS2/PSP), `--format wii`,
`--format final-exam` and `--format xbox360` force one layout (useful for XML
files written by hand). `pc`, `ps2` and `psp` are accepted as aliases for
`obscure2`, and `x360` and `xbox` for `xbox360`.

The Final Exam pool cipher is reversible without a key: the keystream depends
only on the node, attribute and pool counts stored in the header.

The Wii node's last 16 bits are not padding: they hold the low half of a 32-bit
attribute index. Reading them as padding would point every node at the wrong
attributes, which is why the Wii layout needs its own branch.


## CRC dictionary

`hashes.json` maps tag/attribute name CRC32s to names. Unknown names are written
as `tag_XXXXXXXX` / `attr_XXXXXXXX` and round-trip unchanged, so a missing name
never loses data. 60 CRCs used by the shipped files are still unnamed (51 of them
Final Exam-only), including the root tag of the font files.

The PSP port adds a handful of attributes that do not exist in the PC release
(`healType`, `healID`, `color0`, `color1`, `x0`, `y0`, `item_x1`, `item_x2`,
`item_y`, `item_dy`, `no_weap`, `show_quantity`); their names are included in
`hashes.json`.

## Text encoding

The string pool is not UTF-8. The game stores text as CP1252 (Windows-1252),
one byte per character, so decoding it as UTF-8 turned every accented character
into `�` (U+FFFD).

- `extract` decodes each value as UTF-8 when the bytes are valid UTF-8, and as
  CP1252 otherwise, so `é`, `à`, `°`, `œ`, `€` and friends come out correctly.
- `build` writes CP1252 when every character fits, and UTF-8 otherwise.
- `--encoding cp1252` forces CP1252 and `--encoding utf8` forces UTF-8.
  Almost every shipped file is CP1252, on PC, PS2, Wii and PSP alike, including
  the dialogue and the `font_nonserif12`/`font_smallfonts7` glyph tables.
  `font_arial14b.xmc` is the one exception: it stores its glyphs as UTF-8, so
  extracting and rebuilding it without edits converts them to CP1252 (same
  characters, one byte each instead of two).

Font glyphs (`<char>`) also carry a `code` attribute holding the Unicode code
point. Some shipped fonts store U+2122 (`™`) as the two bytes `84 A2`, which is
the UTF-8 sequence `E2 84 A2` with its lead byte missing. `extract` rebuilds the
character from `code` when the stored bytes are exactly that truncated tail, and
`build` writes `84 A2` back, so the round trip stays lossless.

## Round-trip notes

- Strings are written one copy per attribute, like the shipped files. Pass
  `--dedup` to reuse identical strings instead.
- Attribute order inside each element is preserved, so a file that is extracted
  and rebuilt without edits keeps the same attributes in the same order.
- The shipped Obscure pools are zero-padded to a 16-byte boundary. The padding
  is reproduced, so a rebuilt file keeps the original pool size.
- The shipped Obscure 2 files contain one attribute record that no node
  references (a null entry with name CRC `0`): PC 18 of 38 files, PS2 17 of 34,
  PSP 15 of 37 and Wii 20 of 39. It is not reproduced, so those rebuilt files
  are exactly 8 bytes shorter. Final Exam has the same dead record in 208 of its
  382 PC files and 209 of its 382 Xbox 360 files.
- Nodes without attributes keep a stale `first_attr` value in the shipped files.
  A rebuild writes `0` there; the game never reads the field when the attribute
  count is `0`.
- Control characters that XML 1.0 cannot represent are written as `\u0012`
  escapes and restored on build.
- Final Exam string pools are zero-padded to a 16-byte boundary and the padding
  is encrypted along with the strings, so the pool keeps its original size. The
  node ordering can differ, because the shipped files do not follow a single
  traversal order and the game addresses nodes by index. All 382 shipped Final
  Exam files keep an identical XML tree.
- Xbox 360 Final Exam uses the same container in big-endian with a 32-bit
  `first_attr`. All 382 shipped files are XML-stable and keep an identical XML
  tree; 23 rebuild byte-for-byte, while the rest differ in node/attribute
  ordering and stale `first_attr` fields (the game addresses nodes by index),
  and 209 also drop the dead record. No new tag or attribute names are needed:
  the Xbox 360 files use exactly the same CRC set as the PC release.
- Wii files keep their node records byte-for-byte (tag, parent, child and
  attribute counts), but the attribute table is rewritten in node order: the
  shipped files scatter it, so only 5 of 39 are already in order. The tree is
  unchanged; all 39 keep an identical XML tree.
- PSP files use the PC layout, so their node records and attribute table order
  are kept as shipped. All 37 are XML-stable, and 36 of 37 keep an identical
  XML tree, with the same `font_arial14b.xmc` UTF-8-to-CP1252 caveat as PC and
  Wii.
- PS2 files use the PC layout too. All 34 are XML-stable, and 33 of 34 keep an
  identical XML tree, with the same `font_arial14b.xmc` caveat. The PS2 release
  adds no new tag or attribute names: every CRC it uses is already in
  `hashes.json`.


