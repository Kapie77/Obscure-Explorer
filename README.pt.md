# Obscure Explorer

Explorador de arquivos de jogo, somente leitura (no estilo do FModel), para **Obscure 1**, **Obscure 2** e **Final Exam**:
navegue pelos `.hvp` e pelos arquivos soltos, procure, visualize e exporte. Nada é recodificado nem reempacotado.

English: [README.md](README.md)

## O que ele abre

| Tipo | Arquivos | O que você tem |
|------|----------|----------------|
| Arquivos | `.hvp` (a pasta inteira do jogo, ou um arquivo só) | cada `.hvp` aparece como uma pasta; árvore e visão em blocos ou lista, como um explorador de arquivos |
| Texturas | `.dic` (Obscure 2 PC), `.dip` (Obscure 1 PC), `.xbr` (Obscure 1 Xbox), `.dic` e `.hvt` (Obscure 2 Wii), `.dic` (PSP), `.dic` e `.hvi` (PS2), `.hvt` (Final Exam PC / PS3 / Xbox 360), `.bmp` `.tga` `.png` `.jpg` | miniaturas, visualizador em tela cheia, exportação em PNG. Os pacotes abrem como pastas |
| Áudio | `.wav` `.mp3` `.ogg` `.flac`, `.xwma` (Final Exam), `.at3` (PSP) | barra de player com busca, volume, anterior / próximo. `.xwma` e `.at3` precisam do ffmpeg |
| Vídeo | `.bik` (Bink), `.ogv`, `.pmf` (PSP) | player em tela cheia com busca e anterior / próximo. Precisa do ffmpeg |
| Modelos 3D | `.zwo` (Obscure 1 e 2) | visualizador 3D: texturizado / sólido / aramado, lista de malhas, status das texturas. Malhas com esqueleto aparecem na pose de repouso |
| Texto | `.txt` `.ini` `.xml` `.csv` `.srt` `.lua` `.cre` `.sub` ... | janela de texto com busca (UTF-8, UTF-16 e Windows-1252). Lua compilado mostra suas strings |
| Texto (convertido) | `.lng` (os três jogos) | texto legível; exporta como `.lng`, `.txt` ou os dois |
| Texto (convertido) | `.xmc` (Obscure 2) | XML; exporta como `.xmc`, `.xml` ou os dois |
| Flash | `.swf` (Final Exam) | janela de resumo e "Play in Ruffle" |
| Dados de animação | `.ani` (Final Exam), `*anim*.zwo` | etiqueta laranja **ANIM**; ainda sem pré-visualização |

Ainda não suportado: esqueletos e animações dos `.zwo`, `.zwo` do PS2 e
arquivos `.hoe`.

## Como começar

1. Compile e rode: `cargo run -p obscure-explorer --release` (um Rust estável recente; o build release abre sem janela de terminal).
2. **Open folder...** e escolha a pasta do jogo: todos os `.hvp` dentro dela são abertos (cada um aparece como uma pasta com o
   nome do arquivo) junto com os arquivos soltos (pasta `data`, `.exe`, ...). **Open HVP...** abre um arquivo só.
3. O jogo e a plataforma são detectados sozinhos (pastas de disco como `PSP_GAME`, executáveis como `default.xbe`,
   `main.dol` ou `SLES_123.45`, arquivos do lançador Steam / GOG, arquivos `.pmf` / `.at3` / `.xbr`, ou o formato do primeiro `.dic`).
   Troque a **Platform** à mão se ele errar: o PC tem as opções Steam, GOG e Retail.
4. Obscure 2: copie a pasta `hashes` do [obscure-hvp](https://github.com/YouKnow-sys/obscure-hvp) para ao lado do programa,
   senão os nomes dos arquivos dentro dos `.hvp` saem errados.
5. Clique nos itens para abrir e use o botão direito para exportar. Ctrl+clique, Shift+clique ou a caixinha do bloco
   selecionam vários; **Export...** grava os arquivos originais (mantendo as pastas) e / ou cópias convertidas (PNG, XML, TXT).

Se o programa travar, ele grava `obscure_explorer_crash.log` ao lado do executável.

## Ferramentas externas

Elas não vêm junto; cada pop-up oferece configurá-las na primeira vez que forem necessárias.

- **ffmpeg** decodifica `.bik`, `.ogv`, `.pmf`, `.xwma` e `.at3`. Windows: baixado automaticamente (build do gyan.dev). Linux: baixa um build
  estático, ou o pop-up mostra o comando de instalação da sua distribuição. Também dá para apontar um ffmpeg que você já tenha.
- **Ruffle** (emulador de Flash gratuito) toca os `.swf` numa janela própria. É baixado das releases dele no GitHub (Windows e
  Linux) ou localizado por você.
- **hashes.json** dá nome às tags e atributos dos `.xmc` (coloque ao lado do programa, ou use "Locate hashes.json..." na
  janela). Sem ele os nomes aparecem como `tag_XXXXXXXX`.

As ferramentas baixadas e os arquivos que lembram onde elas estão ficam ao lado do programa, ou em
`~/.local/share/obscure-explorer` quando essa pasta é somente leitura.

## Modelos 3D

Clique num `.zwo` para abrir o visualizador: arraste para girar, botão direito para mover, roda para zoom, **F** para
enquadrar e **Esc** para fechar. As texturas são procuradas no `.dic` / `.dip` ao lado do modelo, depois na pasta acima
(personagens do Obscure 2: `boy1/skins/boy1_pc.zwo` usa `boy1/boy1_pc.dic`) e por fim nos pacotes compartilhados na raiz
de cada HVP (armas e itens usam `datapack/texpc_pc.dic`).

## Teclado

O painel sob o ponteiro do mouse é o que recebe o teclado.

- Itens: setas ou W/A/S/D movem o cursor, Enter abre, Ctrl+A seleciona tudo, Esc limpa a seleção, Backspace sobe uma pasta.
- Árvore de pastas: cima / baixo movem, direita expande ou abre um pacote, esquerda recolhe ou vai para a pasta acima.
- Áudio / vídeo: Espaço toca ou pausa; no vídeo, esquerda / direita avançam 5 s e N / P trocam de vídeo.
- Visualizador de imagens: esquerda / direita (ou A / D) trocam de imagem, a roda dá zoom, arrastar move, duplo clique alterna ajustar / 100%.
- Janelas de texto: Ctrl+F busca, Enter / Shift+Enter (ou F3) pulam entre as ocorrências.

## Estrutura do projeto

| Crate | Para que serve |
|-------|----------------|
| `crates/core` | trait `Archive`, pastas e `.hvp`, árvore de arquivos, contexto de jogo / plataforma |
| `crates/textures` | decodificadores de textura (`.dic`, `.dip`, `.hvt`, BC1/2/3, imagens comuns) |
| `crates/models` | leitor de `.zwo` (materiais, malhas, instâncias; o que não é usado é pulado) |
| `crates/xmc` | `.xmc` para XML, mais um leitor de JSON pequeno |
| `crates/lng` | `.lng` para texto (Obscure 1, Obscure 2, Final Exam) |
| `crates/swf` | resumo do cabeçalho e das tags de `.swf` |
| `crates/explorer` | o aplicativo em egui / eframe 0.29 |

## Créditos e licenças

- Arquivos: [obscure-hvp](https://github.com/YouKnow-sys/obscure-hvp), de YouKnow-sys.
- Texturas: os formatos vêm do [ObsCure-Texture-Editor](https://github.com/HeitorSpectre/ObsCure-Texture-Editor), de
  HeitorSpectre (MIT; com Evil Trainer e marcos_haf), e do [obscure-texture-tool](https://github.com/Kapie77/obscure-texture-tool),
  de Kapie77 (GPL-3.0).
- `.xmc`: port do [Obscure-XMC-Converter](https://github.com/Kapie77/Obscure-XMC-Converter), de Kapie77 (GPL-3.0-or-later).
- `.lng`: port do código de extração do [ObsCure-Text-Editor](https://github.com/HeitorSpectre/ObsCure-Text-Editor), de HeitorSpectre.
- `.zwo`: layout do arquivo do [zwoBlender](https://github.com/Al-Hydra/zwoBlender), de Al-Hydra, conferido com arquivos reais do jogo.
- Flash: [Ruffle](https://ruffle.rs). Conversão de vídeo / áudio: [FFmpeg](https://ffmpeg.org).

O projeto inclui código portado de programas GPL-3.0 do próprio autor, então a ideia é distribuí-lo sob GPL-3.0-or-later.
Confira as licenças das outras fontes listadas acima antes de redistribuir builds.
