
Prism Engine Native — Plano Mestre (Zero-WebView Desktop)

Objetivo: Superar o Wallpaper Engine (C++ / DirectX 11) com um motor de área de trabalho 100% nativo em Rust + wgpu (DirectX 12) + Win32 puro, eliminando completamente o processo msedgewebview2.exe do plano de fundo e deixando a arquitetura pronta para futuros ports para macOS (Metal) e Linux (Vulkan).

1. Princípios Arquiteturais

1. Separação Total (Estúdio vs. Engine):
• O Estúdio (apps/prismview-studio) mantém Tauri + React + Three.js para edição visual rápida e rica. Abre apenas sob demanda; quando fechado, consome 0 MB de RAM e 0% de CPU.
• A Desktop Engine (crates/prismview-engine) é um processo/thread nativa em Rust puro acoplada ao WorkerW do Windows. Zero HTML, zero JavaScript, zero V8. Meta: 25 a 55 MB de RAM e ~0,0% de CPU ociosa.
2. Contrato Único (.prism v2):
• O Estúdio exporta um pacote/manifesto JSON + assets otimizados. O motor nativo lê esse contrato de forma determinística.
3. Zero-Copy na GPU:
• Vídeo (Windows Media Foundation -> D3D11 shared texture -> wgpu), texturas 3D (KTX2/BC7) e fontes (glyphon) nunca passam pela CPU quadro a quadro.
4. Sensores e Loop Sob Demanda:
• O loop wgpu entra em sono profundo (ControlFlow::Wait) quando a cena é estática, coberta por janela maximizada, em jogo tela cheia ou na bateria.
5. Determinismo e Zero Drift:
• Toda modificação de sinal/binding sobre animação .glb é aditiva e reversível antes do cálculo de pose do frame.

2. Arquitetura Multiplataforma Desacoplada (crates/prismview-engine)

Todo código específico de sistema operacional fica isolado atrás de Traits (interfaces) na pasta platform/. O núcleo gráfico (wgpu + WGSL) é 100% agnóstico de sistema operacional:
• Pasta core/ (100% Multiplataforma — Zero código de OS aqui):
	• gpu.rs: wgpu (DirectX 12 no Windows, Metal no macOS, Vulkan no Linux)
	• glass.rs: Shaders WGSL de vidro óptico e pirâmide de blur
	• scene3d.rs: glam + gltf + PBR/IBL + skinning
	• text.rs: glyphon + cosmic-text para fontes e relógios na GPU
	• signals.rs: Barramento de sinais lock-free
	• governor.rs: Máquina de estados de energia
• Pasta platform/ (Camada de abstração por Sistema Operacional):
	• mod.rs: Define as traits DesktopHost, HardwareVideoDecoder e SystemSensors
	• windows/: [FASES 0 A 5] WorkerW, Media Foundation D3D11, WASAPI, WM_INPUT, SetWinEventHook
	• macos/: [FUTURO] NSWindow (kCGDesktopWindowLevel), VideoToolbox, CoreAudio
	• linux/: [FUTURO] Wayland (wlr-layer-shell) / X11, VAAPI/GStreamer, PipeWire
Regra de Ouro Multiplataforma:
• Nunca importar código específico de Windows dentro da pasta core/.
• Toda comunicação entre o sistema operacional e o renderizador acontece exclusivamente através das traits definidas em platform/mod.rs.

3. Regras Obrigatórias de Código Rust & Orçamentos

Regras Anti-Bug:
• Zero unwrap() ou expect() no loop de renderização: falha de asset ou dispositivo perdido (SurfaceError::Lost / Outdated) deve reconstruir a swapchain ou usar fallback sem derrubar o processo.
• Zero alocação dinâmica por quadro: Vec, HashMap e String são pré-alocados no Runtime e reaproveitados via .clear().
• Texturas e Buffers: toda textura de vídeo ou render target intermediário (blur pyramid) é recriada apenas no evento Resized ou troca de asset.
• Proibido cópia CPU de vídeo: nunca converter BGRA para RGBA na CPU. Usar formato nativo Bgra8UnormSrgb / Bgra8Unorm direto na wgpu::Texture ou shader de conversão na GPU.
Metas de Consumo (Critério de Aceite por Fase):
• Wallpaper Imagem + Relógio + Vidro Óptico: menos de 45 MB de RAM, 0.0% a 0.1% de CPU ociosa, menos de 2% de GPU.
• Wallpaper Vídeo 4K 60 FPS (Hardware Decode): menos de 65 MB de RAM, menos de 0.5% de CPU, menos de 5% no Video Engine da GPU.
• Cena 3D .glb + Áudio Reativo + Vidro: menos de 120 MB de RAM, menos de 1.5% de CPU, menos de 10% de GPU.
• Jogo em Tela Cheia ou Monitor Desligado: menos de 30 MB de RAM (hibernado), 0.0% de CPU e 0.0% de GPU (loop totalmente parado).

4. Fases de Implementação (Execução Sequencial)

Instrução de uso: Executar 1 Fase por sessão no Claude Opus (High). Validar compilação (cargo check --workspace e cargo clippy) no terminal antes de avançar para a próxima fase.

Fase 0 — Workspace, Traits Multiplataforma, Win32 (WorkerW) e wgpu Base

• Objetivo: Fazer o crates/prismview-engine compilar no workspace principal, criar as traits em platform/mod.rs, abrir a janela acoplada atrás dos ícones do desktop (Progman -> WorkerW) por monitor no Windows e limpar a tela com wgpu (DirectX 12) a 0% de CPU quando parado.
• Entregas:
	• Incluir crates/prismview-engine nos members do Cargo.toml raiz e conectar IPC/comando do src-tauri para iniciar/parar o motor nativo.
	• Criar platform/mod.rs com as traits DesktopHost, HardwareVideoDecoder e SystemSensors (deixando placeholders comentados para macOS e Linux).
	• Implementar platform/windows: descoberta segura do WorkerW no Windows 10/11, suporte a multi-monitor com DPI independente e SetWinEventHook para detectar janelas maximizadas/tela cheia sem polling pesado.
	• Implementar core/gpu.rs com wgpu (Backends::DX12, PowerPreference::LowPower padrão com opção HighPerformance).

Fase 1 — Compositor 2D, Fontes (glyphon) e Vídeo Zero-Copy

• Objetivo: Substituir completamente o WebView2 para wallpapers 2D, relógios, widgets de texto e vídeos em 4K.
• Entregas:
	• Pipeline 2D em WGSL: camadas ordenadas por Z-index, transformações, opacidade, máscaras e bordas arredondadas (SDF).
	• Texto e Relógio nativos em core/text.rs usando glyphon + cosmic-text (suporte às fontes customizadas do projeto, atualização dirty-flag apenas quando o texto/segundo muda).
	• Vídeo por Hardware: substituir o decoder antigo de CPU por Windows Media Foundation entregando frames direto via D3D11 texture sharing para o wgpu (zero cópia na CPU).

Fase 2 — Vidro Óptico Nativo (Shaders WGSL)

• Objetivo: Portar os shaders de vidro óptico, refração, dispersão cromática e pirâmide de blur para WGSL nativo com qualidade superior e custo mínimo.
• Entregas:
	• core/glass.rs: pirâmide de mip/blur (Kawase / Dual-Filter blur em resolução reduzida 1/4 e 1/8) cacheada por hash de conteúdo e frame_token.
	• Refração em tempo real sobre vídeo e 3D usando mapas normais/SDF gerados na GPU (sem Sobel na CPU).

Fase 3 — Motor 3D Nativo (glam + gltf + PBR/IBL)

• Objetivo: Carregar os modelos .glb e cenas multi-objetos do .prism diretamente no wgpu sem Three.js.
• Entregas:
	• Loader assíncrono de .glb / .gltf com suporte a malhas comprimidas (meshopt), texturas KTX2 (basis-universal) e hierarquia de nós (parentId).
	• Pipeline PBR metálico/rugoso com iluminação direcional, pontual, ambiente procedural/HDRI (PMREM gerado via Compute Shader WGSL) e frustum culling.
	• Animação de esqueletos (skinning na GPU via storage buffers) e morph targets.

Fase 4 — Barramento de Sinais (SignalBus) e Bindings Reativos

• Objetivo: Conectar áudio, mouse, sistema e mídia aos objetos 3D, shaders e camadas 2D em tempo real.
• Entregas:
	• core/signals.rs: SignalBus lock-free alimentado por platform/windows: WASAPI loopback FFT (64 bandas + detecção de beat), WM_INPUT (mouse global sem hook lento), SMTC (música atual + cor dominante da capa) e telemetria (CPU/GPU/Bateria).
	• BindingSolver com pipeline aditivo drift-free (desfaz offset/rotação do frame anterior -> atualiza animação -> reaplica binding).

Fase 5 — Governador de Energia (PowerGovernor) e Estúdio Híbrido

• Objetivo: Automação total de economia de bateria/GPU e integração limpa com o Estúdio React.
• Entregas:
	• core/governor.rs: máquina de estados com histerese: Normal, Eco (30 FPS, sem pós-processamento), Ultra (15 FPS), Pausado (jogo tela cheia / monitor desligado) e Congelado (1 frame estático).
	• Hibernação de VRAM: após 60s em Pausado, libera render targets pesados mantendo o estado pronto para retomada instantânea.
	• Integração Tauri: ao clicar em "Aplicar no Desktop" no Estúdio, salva o .prism otimizado e notifica o prismview-engine via Named Pipe / IPC local; o Estúdio pode ser fechado completamente sem afetar o wallpaper.

5. Como Iniciar no Claude Opus (High)

1. Salve este texto no arquivo PLANO_NATIVO.md na raiz do repositório.
2. Abra uma Nova Sessão no Claude Opus (High).
3. Envie o comando inicial:
"Leia o PLANO_NATIVO.md e inspecione a estrutura atual de Cargo.toml, crates/prismview-engine e apps/prismview-studio/src-tauri. Prepare um plano de execução técnico detalhado para a Fase 0 sem alterar código ainda, listando os arquivos e dependências exatas."
