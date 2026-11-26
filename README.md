<div align="center">

# ◈ PrismView

### Seu desktop não precisa ser um depósito de ícones. Pode ser um lugar vivo.

*Wallpapers animados, widgets de vidro e cenas em 3D, feitos por você, rodando no seu próprio computador.*

[Licença: GPL-3.0](LICENSE) · Só Windows 10/11

</div>

---

## O que é

O PrismView é um programa para Windows que troca o papel de parede parado por um **papel de parede vivo**. Ele tem duas partes:

- **Studio**: o editor. Você monta o wallpaper camada por camada (texto, imagem, vídeo, vidro, 3D, efeitos) e vê o resultado na hora.
- **Motor**: o programa leve que desenha o wallpaper atrás dos ícones do desktop. Ele é nativo (Rust + DirectX 12, sem navegador embutido), então gasta pouco e continua rodando mesmo com o Studio fechado.

Tudo roda na sua máquina. Sem conta, sem nuvem, sem coleta de dados.

## Recursos

- **Wallpapers vivos**: vídeo, imagem, composições e cenas 3D, em um ou em todos os monitores.
- **Widgets**: relógio, data, barras, ícones e capa da música que flutuam sobre o desktop, onde você quiser.
- **Vidro óptico**: material translúcido com desfoque, refração, dispersão de cor e brilho.
- **Cenas 3D**: modelos `.glb`/`.gltf` com luz realista, animações, esqueleto e a "Sala 3D" para posicionar tudo com o mouse.
- **Reage ao mundo**: música (graves, médios, agudos, batida), uso de CPU/GPU/RAM, bateria, hora do dia, mouse e teclado.
- **Fórmulas ƒx**: textos e valores que mudam sozinhos (hora, música tocando, clima da máquina etc.).
- **Economia de energia**: pausa quando uma janela cobre o desktop, baixa o ritmo na bateria e tem modos Eco/Ultra/Congelado.
- **Pacotes `.prism`**: tudo que você cria vira um arquivo para guardar, trocar ou presentear.

## Modo Espectro

O Modo Espectro é para quem quer ir além dos blocos prontos: você escreve um **efeito próprio em WGSL** (a linguagem de shader do WebGPU) e ele vira uma camada do wallpaper.

- Abra pelo ícone de arco-íris no topo do Studio.
- Escreva uma função `effect(uv)` que devolve uma cor. O editor mostra erros com a linha certa.
- Crie controles com comentários, por exemplo `//@param brilho: float = 0.5 range(0,1)`. Eles aparecem como sliders no painel.
- Use o que está atrás (`sample_backdrop`), o quadro anterior (`sample_previous`), suas próprias texturas (`//@texture`), simulação (`simulate`) e até **partículas** (`//@particles`, até 200 mil) com física pronta (gravidade, vento, força do mouse, quicar nas bordas).
- O efeito recebe hora, mouse, clique, música e telemetria.
- Exporte e importe efeitos como arquivos `.prismfx`.

O mesmo código roda na prévia do Studio (WebGPU) e no motor do desktop.

## Como instalar

Baixe o instalador na página de [Releases](../../releases) (quando houver uma versão publicada) e execute. Ele instala só para o seu usuário, sem pedir administrador.

Requisitos: Windows 10 ou 11 (64 bits) e uma placa de vídeo com DirectX 12.

## Como compilar

### O que você precisa

- [Node.js](https://nodejs.org/) 22.18 ou mais novo
- [Rust](https://rustup.rs/) estável (alvo `x86_64-pc-windows-msvc`)
- Visual Studio Build Tools com "Desenvolvimento para desktop com C++"
- WebView2 (já vem no Windows 11)

### Passos

```bash
git clone https://github.com/Andy-lucas7/PrismView.git
cd PrismView/apps/prismview-studio
npm install

npm run tauri dev      # abre o Studio em modo de desenvolvimento
npm run installer      # gera o instalador (.exe)
```

O `npm run tauri dev` também compila o motor (`prismview-engine`) e o coloca ao lado do Studio.

### Testes

```bash
npm test                          # testes do Studio (TypeScript)
cargo test -p prism-core          # núcleo compartilhado
cargo test -p prismview-engine    # motor nativo
```

### Estrutura

| Pasta | O que tem |
| --- | --- |
| `apps/prismview-studio/` | Studio (React + Three.js) |
| `apps/prismview-studio/src-tauri/` | Lado Rust do Studio (Tauri 2) |
| `crates/prismview-engine/` | Motor nativo do wallpaper (wgpu + Win32) |
| `crates/prism-core/` | Formato das cenas, fórmulas ƒx, pacote `.prism` |
| `crates/prism-vstem-wasm/` | Alongamento de texto em WebAssembly |

## Licença

O PrismView é software livre, sob a **GNU General Public License v3.0 ou posterior** ([LICENSE](LICENSE)).
As fontes incluídas (Outfit, Inter, JetBrains Mono) seguem a SIL Open Font License. Os decodificadores Draco/Basis em `public/decoders` seguem a Apache-2.0.
