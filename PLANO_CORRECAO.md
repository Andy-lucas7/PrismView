# PLANO_CORRECAO — PrismView (Studio + motor nativo)

- Repo: `Andy-lucas7/PrismView` · base: `f7f7a03` · data: 2026-11-23
- Abreviações: `S/` = `apps/prismview-studio/src/` · `T/` = `apps/prismview-studio/src-tauri/src/` · `E/` = `crates/prismview-engine/src/` · `C/` = `crates/prism-core/src/`
- **As linhas deslocam após cada etapa.** Sempre reencontre o ponto com o `grep` indicado antes de editar.
- Escopo: complementa o `AUDITORIA.md` (itens TS/3D/vidro antigos, a maioria já aplicada). Nada aqui repete aquele arquivo, salvo quando indicado.

---

## Regras globais (agente executor)

1. Nunca ler: `node_modules`, `target`, `dist`, `.git`, `gen/`, lockfiles, binários, fontes, imagens, `_backup_antes_sala3d/`.
2. Achar com `grep -n`, ler só o intervalo (`sed -n 'A,Bp'`). Arquivo inteiro só se < 200 linhas.
3. Só diffs pontuais. Proibido TODO, mock, placeholder, `catch {}` novo ou `let _ =` em I/O novo.
4. Motor: zero `unwrap/expect` no loop, zero alocação por quadro, `dispose`/drop de recursos GPU descartados (regras do `CLAUDE.md`).
5. Ao fim de cada etapa: `npm run build && npm test` (em `apps/prismview-studio`) + `cargo check -p prismview-engine -p prismview-studio --target x86_64-pc-windows-msvc` + `cargo test -p prismview-engine` onde houver testes. **Um commit por etapa.**
6. Mudança em `C/fx.rs` exige a mesma mudança em `S/engine/fx/fxEngine.ts` + `C/fx_parity_cases.json`.
7. Mudança de schema IPC (`C/schema.rs`) exige atualizar `T/native_engine.rs` (monta JSON à mão).

## Ordem de execução (justificativa)

| # | Etapa | Por quê nesta posição |
|---|---|---|
| 0 | Compilar o motor | Fases 1–5 e Lotes 5B–7 foram escritos **sem compilar** (`CLAUDE.md`). Corrigir em cima de código que não compila desperdiça tokens. |
| 1 | Perda de dados, segurança, processos duplicados | Danos irreversíveis (projeto perdido, exe remoto executado, 2 motores na GPU). |
| 2 | Bugs de fluxo e WYSIWYG Studio ↔ motor | O desktop mostra coisa diferente do preview, ou nada, sem aviso. |
| 3 | UX e acessibilidade | Depende dos erros/avisos expostos na Etapa 2. |
| 4 | Desempenho / RAM / VRAM do motor | Exige medição antes/depois; só faz sentido com o motor correto. |
| 5 | Refatoração estrutural | Mexe em arquivos grandes; por último para não invalidar as linhas das etapas anteriores. |

---

## Etapa 0 — Base compilável

**0.1 [Verificar] Motor nunca compilado (todo o motor nativo é suspeito até compilar)**
Onde: `E/**`, `C/**`; `Cargo.toml` raiz (`wgpu = "24"`; o `CLAUDE.md` ainda diz 23).
Causa: `CLAUDE.md` registra "Nada disso foi compilado ainda". APIs marcadas como "a confirmar": `Device::as_hal::<Dx12>`, `hal::dx12::Device::texture_from_raw`, `create_texture_from_hal` (`E/platform/windows/video.rs`), `meshopt::ffi`, `basis-universal 0.3`, `glyphon 0.8` (exige wgpu 24, compatível), `resvg 0.45`.
Correção: rodar `cargo check -p prismview-engine --target x86_64-pc-windows-msvc` numa máquina Windows e corrigir só erros de compilação, sem mudar comportamento. Atualizar `CLAUDE.md` (wgpu 24). Os comentários de `crates/prismview-engine/Cargo.toml` estão com mojibake (UTF-8 duplo, ex. `VERSÃ•ES`): reescrever só os comentários.
Aceite: `cargo check` e `cargo test -p prismview-engine` passam; `cargo build --release` gera `prismview-engine.exe`.

---

## Etapa 1 — Perda de dados, segurança, processos

**1.1 [Confirmado] Salvar projeto não é atômico e a falha é silenciosa (projeto corrompido ou perdido)**
Onde: `T/lib.rs:1127-1135` (`grep -n "fn save_project_inner" T/lib.rs`); `S/App.tsx:582, 658, 697` (`grep -n 'invokeCmd("save_project"' S/App.tsx`), além de `S/App.tsx:417`.
Causa: `fs::write(project_path(id), json)` escreve direto no arquivo final. Crash, queda de energia ou disco cheio no meio da escrita deixam um JSON truncado. No front, todo `save_project` termina em `.catch(() => {})`.
Cenário: editar → disco cheio / pasta somente leitura / OneDrive bloqueando → a UI não mostra nada → fechar → o projeto volta antigo ou vazio.
Correção:
```rust
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    { let mut f = File::create(&tmp).map_err(|e| e.to_string())?;
      f.write_all(bytes).map_err(|e| e.to_string())?; f.sync_all().map_err(|e| e.to_string())?; }
    if path.exists() { let _ = fs::copy(path, path.with_extension("json.bak")); }
    fs::rename(&tmp, path).map_err(|e| e.to_string()) // rename substitui no Windows (MoveFileEx REPLACE)
}
```
Usar o helper em `save_project_inner`, `save_session_from_state`, `export_prism_archive_sync` (1.6) e `native_engine.rs` (power/tray). No TS, criar `saveProjectOrReport(p)`: em erro, manter `pendingSaveRef` sujo, mostrar toast "Não foi possível salvar: <motivo>. Tentaremos de novo." e reagendar com backoff (2 s, 5 s, 15 s).
Aceite: matar o processo durante o save nunca deixa `.json` inválido; com a pasta somente leitura, o toast aparece e as edições continuam em memória.

**1.2 [Confirmado] `session.json` não atômico e com erro engolido (widgets fixados somem)**
Onde: `T/lib.rs:599-634` (`grep -n "fn save_session_from_state" T/lib.rs`), escrita na linha 631; leitura em `T/lib.rs:636-639`.
Causa: `let _ = fs::write(session_path(), json)`. JSON truncado em `load_persisted_session` cai em `.ok()?`, vira "sem sessão" e o próximo save grava a sessão vazia por cima.
Cenário: desligar o PC enquanto um widget é arrastado (o save é disparado por `dirty_widget_saves`) → no boot os widgets não voltam.
Correção: `write_atomic` + `eprintln` → log em arquivo (ver 2.2). Na leitura, se o parse falhar, renomear para `session.corrupt-<ts>.json`, tentar o `.bak` e só então assumir vazio.
Aceite: um `session.json` truncado à mão é recuperado do `.bak`.

**1.3 [Confirmado] Importar `.prism` com o mesmo `id` sobrescreve o projeto do usuário sem perguntar**
Onde: `T/lib.rs:2139-2230` (`grep -n "async fn import_prism_archive" T/lib.rs`), `save_project_inner(val)` na linha 2222; `S/App.tsx:817-838`.
Causa: o `id` vem do pacote. Se já existe `projects/<id>.json`, ele é substituído e as pastas `imports/` antigas são limpas. Importar duas vezes o mesmo template, ou receber de volta um wallpaper exportado, apaga as edições locais.
Cenário: exportar "Meu WP" → continuar editando → importar o `.prism` antigo → as edições somem.
Correção: no Rust, se `project_path(&proj_id).is_file()`, gerar `Uuid::new_v4()` como novo `id` e anexar " (importado)" ao `title`, a menos que o front passe `replace: true`. No front, quando houver colisão (o comando devolve `{ conflict: true, existingTitle }`), mostrar o `ConfirmDialog` com "Substituir" / "Manter os dois" (padrão). Se o projeto substituído estiver aberto no editor, atualizar `editing` e limpar `pendingSaveRef` desse id, senão o autosave reescreve a versão velha.
Aceite: importar o mesmo arquivo duas vezes gera dois projetos; "Substituir" exige confirmação.

**1.4 [Confirmado] Projeto com JSON inválido some da lista sem aviso**
Onde: `T/lib.rs:1104-1125` (`grep -n "async fn list_projects" T/lib.rs`).
Causa: falha de parse é ignorada (`if let Ok`). O usuário acha que perdeu o projeto e cria outro com o mesmo id, ou um template padrão sobrescreve (`S/App.tsx:417` salva os defaults).
Correção: devolver `{ projects, broken: [{ file, error }] }`. Mover o arquivo inválido para `projects/_corrompidos/` e tentar o `.bak` de 1.1. No Hub, mostrar a faixa "1 projeto não pôde ser aberto — Ver detalhes".
Aceite: um JSON truncado à mão aparece no aviso e é restaurado do `.bak` quando houver.

**1.5 [Confirmado] Pacote importado pode executar qualquer programa, inclusive em compartilhamento de rede, com um clique no desktop**
Onde: `T/native_scene.rs:417-419` (`grep -n '"OPEN_APP"' T/native_scene.rs`) → `E/native.rs:812-833` (`run_touch_action`) → `E/platform/windows/shell.rs:14-43`.
Causa: `clickTarget` do projeto (vindo de um `.prism` de terceiros) vai direto para `ShellExecuteW("open")`. Não há validação de caminho UNC (`\\host\share\x.exe`), `.bat/.cmd/.ps1/.vbs/.lnk/.url`, nem confirmação.
Cenário: baixar um wallpaper `.prism` → aplicar → clicar no relógio do desktop → roda `\\evil\s\a.exe`.
Correção:
1. No import (`T/lib.rs`, após o parse), percorrer as camadas. Se houver `clickAction == "OPEN_APP"`, zerar `clickTarget` e marcar `"clickTargetNeedsReview": true`. O front lista essas camadas num diálogo pós-import ("Este pacote quer abrir programas: … Permitir?").
2. Em `shell_open(target, false)`: recusar caminhos UNC/`\\?\`, exigir `Path::is_absolute()` + `is_file()` local e extensões permitidas `exe|lnk|appref-ms`. URLs já são filtradas.
Aceite: um pacote com `OPEN_APP` UNC importa com a ação desativada; `shell_open` recusa `\\host\x.exe`.

**1.6 [Confirmado] Import libera no asset protocol qualquer arquivo absoluto citado no JSON**
Onde: `T/lib.rs:561-575` (`grep -n "fn allow_project_media" T/lib.rs`), chamado em `list_projects`, `load_project_from_disk` e `import_prism_archive`.
Causa: a função percorre **todas** as strings do projeto. Qualquer caminho absoluto existente (`C:\Users\x\Documents\senhas.xlsx`) entra no escopo do `asset:` do WebView. Um pacote malicioso pode citar arquivos do usuário. O risco de exfiltração é baixo com a CSP atual, mas a superfície é desnecessária.
Correção: liberar só os campos de mídia conhecidos (`video.src`, `image.src`, `text.fontPath`, `scene3d.objects[].modelSrc`, `scene3d.envMapSrc`, `svg.*Src`) e só as extensões de mídia/fonte/modelo. Reaproveitar a lista de `remap_imported_layers` (`T/lib.rs:~1051`).
Aceite: um projeto com `"title": "C:\\Windows\\win.ini"` não libera o arquivo no escopo.

**1.7 [Confirmado] Export `.prism` não atômico, inclusive o automático em background**
Onde: `T/lib.rs:2080-2082` (`grep -n "File::create(&dest)" T/lib.rs`); chamado a cada Aplicar/autosave por `T/native_engine.rs:219-262`.
Causa: `File::create(dest)` trunca o pacote existente antes de reescrever. Se o Studio fechar durante o zip (vídeos grandes levam segundos), `packages/<id>.prism` fica corrompido.
Correção: gravar em `dest.with_extension("prism.tmp")` e fazer `rename` ao final; em erro, apagar o `.tmp`.
Aceite: matar o Studio durante o export mantém o `.prism` anterior íntegro.

**1.8 [Confirmado] Dois motores podem rodar ao mesmo tempo (GPU/RAM em dobro, janelas sobrepostas no WorkerW)**
Onde: `E/native.rs:159-197` (`run`, sem mutex), `E/ipc_server.rs:31-62` (uma instância de pipe por vez), `T/native_engine.rs:37` (`PIPE_BUSY_TIMEOUT = 150ms`), `T/native_engine.rs:77-90, 343-391`.
Causa: (a) o motor não tem guarda de instância única. (b) O servidor só cria a próxima instância do pipe depois de terminar o cliente atual, e um `LOAD_SCENE_FILE` pode ocupar a instância por até 3 s (ver 1.10). (c) Nesse intervalo, o `PING` do Studio recebe `ERROR_PIPE_BUSY` por mais de 150 ms, conclui "motor fora do ar" e chama `Command::spawn` de outro motor. Também há corrida entre o login (`HKCU\Run\PrismViewEngine`) e o Studio `--minimized` subindo junto.
Cenário: Aplicar um wallpaper com vídeo 4K enquanto o Hub consulta o status → um segundo `prismview-engine.exe` aparece no Gerenciador de Tarefas.
Correção:
1. Em `run()`, antes do host: `CreateMutexW(None, true, w!("Local\\PrismViewEngine"))`. Se `GetLastError() == ERROR_ALREADY_EXISTS`, sair com código 0. Confirmar a assinatura em `windows 0.58` (`Win32::System::Threading`).
2. Em `ipc_server.rs`: criar a instância seguinte **antes** de atender o cliente (uma thread por conexão, ou criar a instância N+1 logo após `ConnectNamedPipe`).
3. Em `pipe_try_once_at`: `ERROR_PIPE_BUSY` = motor vivo. Usar `WaitNamedPipeW(PIPE_PATH, 2000)` e nunca disparar spawn com base em BUSY.
Aceite: com o motor no ar, `native_engine_start` repetido 20× em paralelo mantém um único processo.

**1.9 [Confirmado] Named pipe aceita clientes remotos e pode ser sequestrado**
Onde: `E/ipc_server.rs:35-44` (`grep -n "CreateNamedPipeW(" E/ipc_server.rs`).
Causa: sem `PIPE_REJECT_REMOTE_CLIENTS`, sem `FILE_FLAG_FIRST_PIPE_INSTANCE` e com DACL padrão. Outro processo pode criar `\\.\pipe\prismview_ipc` antes do motor (squatting) e responder ao Studio. Um cliente via SMB pode mandar `LOAD_SCENE_FILE` com um caminho arbitrário.
Correção: flags `PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE` (só na 1ª instância) e `PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS`. Usar `SECURITY_ATTRIBUTES` com SDDL `D:P(A;;GA;;;OW)(A;;GA;;;SY)` via `ConvertStringSecurityDescriptorToSecurityDescriptorW` (confirmar os nomes em `windows 0.58`: `Win32::Security::Authorization`, feature `Win32_Security_Authorization` no `Cargo.toml` do motor). Em `LOAD_SCENE_FILE`, aceitar só caminhos dentro de `%APPDATA%\PrismView\native\`.
Aceite: com a 1ª instância já criada por outro processo, o motor falha e registra em log em vez de servir; um `LOAD_SCENE_FILE` com `C:\x.json` é recusado.

**1.10 [Confirmado] O loop do motor bloqueia em I/O pesado; o IPC responde "engine_timeout" com o trabalho ainda em curso**
Onde: `E/ipc_server.rs:76` (`recv_timeout(3s)`); `E/native.rs:635-671` (`refresh_videos` → `decoder.open` síncrono, `OPEN_TIMEOUT = 10s` em `E/platform/windows/video.rs:85`); `E/core/scene2d.rs:219-246` (`image::open` dentro do rebuild); `E/core/text.rs:257-285` (`load_system_fonts` no 1º nome de fonte não encontrado).
Causa: tudo roda na thread do loop, que também atende o IPC, os sinais e o render. Uma cena com vídeo HEVC 4K + imagem 8K trava o wallpaper (todos os monitores) por segundos, e o Studio mostra "Não foi possível aplicar" embora a cena apareça depois.
Correção:
1. `MfVideoDecoder::open` não bloqueante: devolver `Pending`, e o worker avisa por `wake` (o canal `OpenReply` já existe). `refresh_videos` só registra os slots.
2. Imagens: decodificar no `AssetLoader` (`E/core/assets/loader.rs`, threads + `catch_unwind` já existem). O rebuild usa a textura quando o evento chegar (`dirty = true`).
3. Fontes do sistema: carregar em thread na criação do `TextSystem`, ou só para o fallback de glifos (ver 4.6).
4. `LOAD_SCENE_FILE` responde `Ok` logo após o parse. Erros de assets vão para o status (2.2).
Aceite: `LOAD_SCENE_FILE` responde em < 50 ms com vídeo 4K + imagem 8K; o relógio de outra camada não para de atualizar durante a carga.

**1.11 [Confirmado] `LOAD_SCENE_FILE` durante um rebuild de saídas é rejeitado e o motor fica com a cena velha**
Onde: `E/native.rs:451-457` (`grep -n "fn load_scene" E/native.rs`).
Causa: `if self.gpu.is_none() || self.outputs.is_empty() → Error("gpu_unavailable")` **antes** de `self.scene = Some(scene)`. O Studio já gravou o novo `active_scene.json`, mas o motor mantém a cena anterior em memória e a reexibe no próximo `rebuild_outputs` (Explorer reiniciando, monitor religando, device perdido).
Correção: sempre `self.scene = Some(scene)`. Se houver renderer, aplicar na hora; senão, `ensure_renderer` aplica no rebuild (já lê `self.scene`). Responder `Ok`. O mesmo vale para `ClearDesktop` (`E/native.rs:~1107`): limpar o estado mesmo sem GPU.
Aceite: reiniciar o `explorer.exe` e aplicar outra cena em seguida mostra a cena nova quando o desktop volta.

**1.12 [Verificar] Fechar o Studio pode perder até ~450 ms de edição**
Onde: `S/App.tsx:630-640` (`grep -n "beforeunload" S/App.tsx`).
Causa: `beforeunload` chama `flushPendingSave()` assíncrono sem `await`, e o WebView pode ser destruído antes do `invoke` chegar ao Rust.
Correção: `getCurrentWindow().onCloseRequested(async (e) => { e.preventDefault(); await flushPendingSave(); await getCurrentWindow().destroy(); })` (confirmar a API em `@tauri-apps/api` 2.x). Manter `beforeunload` só como fallback para o navegador.
Como verificar: mover um slider e fechar com Alt+F4 em < 450 ms; reabrir e comparar.
Aceite: o valor do slider persiste.

**1.13 [Verificar] Sair do Windows ou encerrar o Studio pode esvaziar a lista de widgets fixados**
Onde: `T/lib.rs:3048-3054` (`grep -n "WindowEvent::Destroyed" T/lib.rs`).
Causa: o `Destroyed` de uma janela `widget-*` remove o widget de `pinned_widgets`. No logoff e no `app.exit` as janelas são destruídas. Se algum `save_session_from_state` rodar depois (thread de fundo, `flush_dirty_widget_saves`), a sessão é salva sem widgets.
Correção: ao receber `ExitRequested`/`allow_exit` ou `WM_QUERYENDSESSION`, marcar `st.shutting_down = true`. Em `Destroyed`, só remover de `pinned_widgets` quando `!shutting_down` e a remoção veio de `unpin_standalone_widget`.
Como verificar: fixar 2 widgets → fazer logoff → logar → os widgets voltam?
Aceite: os widgets voltam após logoff e após "Sair" pela bandeja.

**1.14 [Confirmado] Studio sem instância única**
Onde: builder em `T/lib.rs:~2990-3117`; `grep -n single_instance T/lib.rs` → vazio.
Causa: abrir o atalho duas vezes cria dois Studios, cada um com bandeja, thread de fundo e autosave próprios no mesmo `projects/<id>.json` (o último a gravar vence).
Correção: `tauri-plugin-single-instance` (2.x; confirmar a versão compatível com o Tauri do projeto). No callback, chamar `open_or_focus_studio(app)` (`T/lib.rs:522`).
Aceite: o segundo clique no atalho só foca a janela existente.

---

## Etapa 2 — Fluxo e WYSIWYG (Studio ↔ motor)

**2.1 [Confirmado] Todos os templates 3D e o "Novo 3D" sem modelo ficam vazios no desktop**
Onde: `E/core/scene3d/renderer.rs:553-554` (`grep -n "SHOWCASE" E/core/scene3d/renderer.rs`); origem em `S/types/prism.ts:1051, 1138, 1994, 2099` (`createScene3DObject("SHOWCASE"`), `create3DTemplateProjects` (`S/types/prism.ts:2194`).
Causa: sem `modelSrc`, o objeto é `SHOWCASE` (escultura procedural de `S/engine/scene3d/sculptures.ts`). O motor marca `slot.error` e não desenha nada. O preview do Studio mostra a escultura; o desktop mostra só o fundo.
Cenário: Hub → template "Obsidian 3D Studio" → Aplicar → a área 3D fica vazia.
Correção: portar `sculptures.ts` para `E/core/scene3d/sculptures.rs`: geração procedural de malha (mesmos parâmetros por `showcaseVariant`, incluindo GYROSCOPE e KINETIC_SLAB) → `ModelAsset` sintético registrado no `AssetVault` com hash `showcase:<variant>`, reaproveitando `gpu_model.rs` e a animação por spin. Ler o `.ts` por intervalo e portar 1:1 as constantes de geometria e material. Até a etapa estar pronta, o aviso de 2.4 deve listar SHOWCASE.
Aceite: os templates 3D aparecem no desktop com a mesma silhueta, material e rotação do preview (comparar print lado a lado).

**2.2 [Confirmado] Erros do motor e do conversor nunca chegam ao usuário**
Onde: `T/native_engine.rs:282-284` (`warnings` → `eprintln!`), `T/native_scene.rs:796-901` (`materialize_assets`: "arquivo ausente"), `E/native.rs` (todos os `eprintln!`, com stderr = `Stdio::null()` em `T/native_engine.rs:~366`), `E/core/scene3d/renderer.rs:620` (`slot.error`), `C/schema.rs` (`EngineStatus` sem erros).
Causa: o motor roda destacado, sem console; os avisos de assets ausentes, vídeo que não abriu, modelo inválido e SHOWCASE são descartados.
Correção:
1. `apply_project` devolve `Vec<String>` de avisos → `apply_desktop_wallpaper` inclui `warnings` no DTO → o front mostra um toast com "Ver detalhes".
2. `EngineStatus.errors: Vec<EngineIssue { layer_id, message }>` (prism-core) preenchido por `Scene3dRenderer` (erros por objeto), `refresh_videos`, `ImageCache.failed` e `read_scene_file`. Lista limitada a 32 itens, limpa no `load_scene`.
3. Log rotativo do motor em `%APPDATA%\PrismView\native\engine.log` (≤ 1 MB, 2 arquivos) no lugar de `eprintln!`. Mesmo padrão no Studio (`T/lib.rs`, 60 `let _`/`.ok()` silenciosos: registrar pelo menos os de I/O).
Aceite: aplicar um projeto com vídeo apagado mostra "Vídeo não encontrado: <nome> (camada X)".

**2.3 [Confirmado] Fontes padrão (Outfit/Inter/JetBrains Mono) vêm do Google Fonts, que a própria CSP bloqueia; o motor não as tem**
Onde: `apps/prismview-studio/index.html:10`, `desktop.html:10` (`grep -n fonts.googleapis`); CSP em `T/../tauri.conf.json:30` (`style-src 'self' 'unsafe-inline'`, `font-src 'self' data: asset:`); `T/native_scene.rs:319` (`font_id` padrão `"Outfit"`); `E/core/text.rs:257-285`.
Causa: o `<link>` para `fonts.googleapis.com` é bloqueado pela CSP (e falha offline). O Studio desenha com o fallback CSS (`Segoe UI`). O motor não acha "Outfit" na pasta `fonts`, carrega todas as fontes do sistema e usa `SANS_FALLBACKS`. Preview e desktop podem usar fontes diferentes da pretendida e diferentes entre si. Há ainda uma requisição externa desnecessária.
Correção: empacotar Outfit, Inter e JetBrains Mono (OFL) como `.woff2` em `public/fonts/` com `@font-face` local para a UI, e como `.ttf` em `bundle.resources`. Na 1ª execução, copiar os `.ttf` para `%APPDATA%\PrismView\fonts\` (o motor já registra essa pasta em `E/native.rs:892-897`). Remover os `<link>` do Google.
Aceite: offline, Studio e desktop desenham "Outfit" idênticos; DevTools sem erro de CSP.

**2.4 [Confirmado] Recursos aceitos no Studio que o motor não suporta, sem aviso: woff2, 2ª camada 3D, SHOWCASE**
Onde: `S/components/StudioEditor.tsx:1233` (`.woff2` no accept), `T/lib.rs:2274` (filtro do diálogo nativo inclui `*.woff2`); `E/native.rs:256-270` (só a 1ª camada 3D: `scene3d_config`); 2.1.
Correção:
- woff2: recomendado converter para TTF no `pick_local_file` (Rust, decoder WOFF2; confirmar um crate mantido, ex. `woff2-patched` ou `wuff`) e salvar em `fonts/`. Trade-off: +1 dependência; a alternativa é remover woff2 do filtro, o que é mais simples mas tira um formato comum.
- Painel "Compatível com o desktop" no Inspector (Etapa 3.4) com a lista calculada por uma função pura `nativeCompatIssues(project)` em `S/utils/nativeCompat.ts`, espelhando os limites do conversor.
- 2ª camada 3D: suportar N camadas no motor (um `Scene3dRenderer` por camada, compartilhando `Scene3dShared` e o `AssetVault`), ou bloquear a adição no Studio. Recomendado suportar N com teto de 2, para não multiplicar alvos HDR/MSAA (ver 4.3).
Aceite: escolher um woff2 funciona no desktop; um projeto com 2 camadas 3D renderiza as duas ou mostra o aviso antes de Aplicar.

**2.5 [Confirmado] Código morto do wallpaper WebView e seletor de monitor sem efeito**
Onde: `T/lib.rs:121` (`wallpaper_labels`, nunca inserido: `grep -n "wallpaper_labels.insert"` → vazio), usos em `1172, 1355-1361, 1412-1419, 1804, 2430, 2584, 3055`; `monitor_wallpapers` (nunca inserido); `wallpaper_window_label` (`T/lib.rs:582`); `T/lib.rs:1487, 1516` (`let _ = monitor_id`); `S/App.tsx:525-532` (ramo `matchingMonitors`, sempre vazio); `targetMonitorId` em `S/App.tsx:708`.
Causa: o Lote 6B removeu o WebView de wallpaper, mas o estado, os laços da thread de fundo e a UI de monitor alvo continuam. É uma opção na UI que não faz nada.
Correção: remover `wallpaper_labels`, `monitor_wallpapers`, `monitor_wallpaper_snapshots` (manter a leitura do campo na sessão para migração), `close_legacy_wallpaper_windows` após a migração, o ramo `matchingMonitors` e o seletor de monitor. Se "um wallpaper por monitor" for desejado, é feature nova do motor (`PrismScene` por `monitor_id`), fora deste plano.
Aceite: `grep -n wallpaper_labels T/lib.rs` → vazio; a UI não oferece monitor alvo.

**2.6 [Confirmado] Cada autosave do wallpaper ativo reconverte, rematerializa e re-zipa todas as mídias**
Onde: `S/App.tsx:534-540, 612` (debounce 450 ms → `syncIfActiveOnDesktop` → `apply_desktop_wallpaper`); `T/native_engine.rs:269-301` + `219-262` (`schedule_background_archive`); `T/lib.rs:2082` (`CompressionMethod::Deflated` para tudo).
Causa: arrastar um slider num wallpaper ativo com vídeo de 300 MB gera, a cada pausa de 450 ms, um zip Deflate de 300 MB em background (CPU alta, desgaste de SSD), além do `materialize_assets`.
Correção:
1. O export `.prism` em `packages/` roda só no "Aplicar" explícito e no "Exportar"; o autosave só atualiza `active_scene.json` + `LOAD_SCENE_FILE`.
2. `CompressionMethod::Stored` para `mp4|webm|mkv|mov|jpg|jpeg|png|webp|glb|ktx2|woff2` (já comprimidos).
3. `materialize_assets`: pular o arquivo se `(len, mtime)` não mudou (cache em memória no `NativeEngine`).
Aceite: com o wallpaper de vídeo ativo, mover um slider por 10 s não gera escrita em `packages/` (Process Monitor); CPU do Studio < 5% após soltar.

**2.7 [Confirmado] Sync automático com o desktop falha em silêncio**
Onde: `S/App.tsx:514-541` (`.catch(() => null)` em `pin_standalone_widget` e `apply_desktop_wallpaper`).
Causa: se o motor caiu ou o conversor falhou, o desktop congela na versão antiga e o usuário segue editando sem saber.
Correção: na 1ª falha seguida, toast "O desktop não pôde ser atualizado: <motivo> — Tentar de novo"; não repetir o toast até um sucesso.
Aceite: matar o motor e editar o wallpaper ativo mostra o aviso uma vez; o próximo sync bem-sucedido o limpa (ou o `ensure_started` religa o motor).

**2.8 [Confirmado] Vídeo com erro: log a cada iteração e vídeo congelado para sempre**
Onde: `E/native.rs:700-702` (`grep -n "last_error()" E/native.rs`); `E/platform/windows/video.rs:1215-1217`.
Causa: `last_error()` faz lock + clone de `String` e o `eprintln!` roda a cada iteração do loop enquanto o erro persistir (alocação por quadro). Não há reabertura.
Correção: `last_error` passa a devolver um `error_generation: u64` barato. `step_videos` loga e publica em `EngineStatus.errors` só quando a geração muda, e agenda reabrir o decoder com backoff (1 s, 5 s, 30 s; máx. 3 tentativas).
Aceite: um arquivo de vídeo corrompido gera 1 linha de log e o aviso no status, sem uso de CPU contínuo.

**2.9 [Verificar] 3D em multi-monitor usa o tamanho da 1ª saída visível**
Onde: `E/native.rs:323-329` (`scene3d_placement`), `E/native.rs:357-359` (`set_viewport`).
Causa: a textura 3D é renderizada no tamanho da caixa do 1º monitor visível e amostrada nos demais; com 1080p + 4K, o 4K recebe a textura ampliada e o aspecto pode divergir.
Como verificar: monitores com resoluções e orientações diferentes, cena 3D full-bleed; comparar a nitidez.
Correção (se confirmado): viewport = maior caixa entre as saídas visíveis, com câmera por aspecto de cada saída, ou um render por saída quando os aspectos diferem mais de 5%.

**2.10 [Confirmado] Hover/clique 3D quebra com a câmera na vertical**
Onde: `E/core/scene3d/renderer.rs:905-909` (`fwd.cross(Vec3::Y)` → zero → `return None`).
Correção: se `|fwd·Y| > 0.999`, usar `Vec3::Z` como up de referência.
Aceite: elevação 90° ainda permite clicar no objeto.

---

## Etapa 3 — UX e acessibilidade (propostas concretas)

**3.1 [Confirmado] Botões só com ícone sem nome acessível; sem foco visível**
Onde: `S/components/StudioInspector.tsx` (54 `<button>`, 0 `aria-label`), `S/components/StudioEditor.tsx` (39 / 0), `S/App.tsx` (39 / 3); `grep -c focus-visible` = 0 nos três.
Correção: todo `<button>` sem texto recebe `aria-label` igual ao `title` (há `title` em parte deles). Classe utilitária global em `S/styles/prism.css`: `:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }`. Inputs: associar `<label htmlFor>` ou `aria-label`.
Aceite: navegar o Inspector só com Tab mostra o foco em todos os controles; o Narrador lê o nome de cada botão.

**3.2 [Confirmado] Animações ignoram "reduzir movimento" do Windows**
Onde: `S/App.tsx:3` (framer-motion); `grep -rn "useReducedMotion\|MotionConfig" S/` → vazio.
Correção: envolver a raiz em `<MotionConfig reducedMotion="user">` (`S/App.tsx`) e `@media (prefers-reduced-motion: reduce) { .animate-pulse { animation: none } }` no CSS.
Aceite: com "Mostrar animações" desligado no Windows, os diálogos aparecem sem transição.

**3.3 [Confirmado] `ConfirmDialog` sem semântica de diálogo nem foco preso**
Onde: `S/App.tsx:122-193`.
Correção: `role="dialog" aria-modal="true" aria-labelledby`, foco inicial no botão seguro ("Cancelar") nas ações destrutivas, Tab circular dentro do diálogo e devolução do foco ao botão de origem ao fechar. Enter confirma só quando o foco está no botão de confirmar.
Aceite: Tab não sai do diálogo; Esc fecha; o foco volta ao botão de origem.

**3.4 Proposta — Indicador de estado de salvamento e de desktop**
O que o usuário vê: no cabeçalho do editor (ao lado do título do projeto), um chip com 4 estados: "Salvando…", "Salvo", "Falha ao salvar · Tentar de novo" (1.1) e "Desktop desatualizado" (2.7). Clicar no chip abre os detalhes.
Padrão: autosave como hoje; sem Ctrl+S obrigatório, mas Ctrl+S força o flush e mostra "Salvo" (atalho esperado no desktop). Registrar em `S/hooks/useKeyboardShortcuts.ts` (`mod && key === "s"` → `onSave`).

**3.5 Proposta — Painel "Compatível com o desktop"**
Onde: topo do `StudioInspector`, colapsável, visível só quando `nativeCompatIssues(project).length > 0` (2.4).
Conteúdo: uma linha por problema ("Escultura procedural ainda não aparece no wallpaper", "Fonte .woff2 não suportada — converter", "Só a primeira cena 3D é desenhada") e um botão "Selecionar camada".
O botão "Aplicar no desktop" mostra um badge amarelo com a contagem. Recomendado: avisar, não bloquear (o usuário pode querer o resto da cena). Trade-off: o usuário pode ignorar o badge; por isso o toast de 2.2 se repete após o Aplicar.

**3.6 Proposta — Diálogo pós-import com permissões** (de 1.3/1.5)
Lista: "Este pacote substitui/duplica", "Este pacote quer abrir: <app>" com checkbox por ação; padrão desmarcado.

**3.7 [Verificar] Contraste e alvos de clique**
Como verificar: Accessibility Insights for Windows no Studio (tema escuro), checando texto secundário (`text-white/40`-like) ≥ 4.5:1 e ícones de 14 px (`w-3.5 h-3.5`) com área clicável ≥ 24 px. Corrigir com `min-w-6 min-h-6` no botão, não no ícone.

---

## Etapa 4 — Desempenho, RAM e VRAM do motor (medir antes e depois)

**Passo obrigatório de medição (antes e depois de cada item):**
- RAM privada / working set: Process Explorer (`prismview-engine.exe`, colunas Private Bytes, WS Private, GPU Dedicated/Shared Bytes) com 3 cenas fixas: (A) texto + relógio, (B) vídeo 1080p full-bleed + vidro, (C) template 3D + parallax.
- CPU ociosa: Process Explorer, aba Threads, CPU cycles delta em 60 s com o desktop visível e o mouse parado, e depois com o desktop coberto.
- Frame time: PIX ou contador interno (`frames` em `GET_STATUS`) por 30 s.
- Registrar os números numa tabela no PR. **Não prometer números sem medir.**

| Consumidor | Estimativa (marcada como estimativa) | Correção |
|---|---|---|
| Rebuild 2D completo a cada frame de vídeo e a cada passo de mola de binding/parallax (4.1) | CPU: N camadas × M monitores por quadro (60–144 Hz); alocações por quadro | Separar estrutura de dinâmica |
| Imagem de fundo no tamanho original, sem mips (4.2) | 6000×4000 RGBA ≈ 96 MB VRAM + 96 MB de pico na CPU vs ≈ 8 MB a 1080p | Reduzir ao tamanho de saída + mips |
| 3D MSAA 4× Rgba16F fixo (4.3) | 4K: ≈ 265 MB MSAA + 133 MB depth + 66 MB HDR + 33 MB saída ≈ 500 MB; 1080p ≈ 124 MB | MSAA por caps/resolução |
| Worker de vídeo com wait de 100 ms (4.4) | 10 wakeups/s por vídeo, inclusive pausado | `Condvar::wait` sem timeout |
| `load_system_fonts` (4.6) | metadados de centenas de fontes + mmap; pico de alguns MB a dezenas de MB (estimativa) | Fontes empacotadas (2.3) e carga só para fallback |
| Studio residente: tick 250 ms + EnumWindows 500 ms (4.5) | 4–6 wakeups/s com o Studio só segurando widgets | Já descrito em `AUDITORIA.md` 6.3/6.4 |

**4.1 [Confirmado] Frame de vídeo e bindings refazem a lista 2D inteira a cada quadro (viola "zero alocação por quadro")**
Onde: `E/core/scene2d.rs:472-478` (`apply_signals` → `self.version += 1`), `555-595` (`set_video_frame` → `version += 1` quando o slot muda, ou seja, todo frame do anel de 5), `783-806` (`render`: `stale` → `rebuild` + `text.flush` + `canvas.flush` + `glass.flush`), `1477` (`format!("{}@{}", layer.id, ...)` por camada de vidro-glifo em cada rebuild), `~900` (`String` do sufixo em cada rebuild).
Causa: `version` mistura "a estrutura mudou" com "um valor dinâmico mudou". Vídeo a 60 fps ou parallax a 144 Hz executam layout, sweep de texto, ordenação e reupload de instâncias por monitor a cada quadro.
Correção:
1. Vídeo: um `TextureId` estável por vídeo no compositor, apontando para uma tabela de 5 bind groups (um por slot do anel). `set_video_frame` troca só o índice ativo (`compositor.set_video_slot(id, slot)`), sem bump de `version`. O batch resolve o bind group no `render_with`.
2. Bindings: `LayerBindingSolver` produz deltas por camada; `Canvas` guarda `layer_id → range de instâncias` no rebuild. `apply_signals` reescreve só `inst.transform/opacity` desses ranges e faz `queue.write_buffer` do trecho (buffer de instâncias pré-alocado). Bump de `version` só para mudanças estruturais (texto mudou, visibilidade, cena).
3. Pré-computar a chave `layer.id@canvas_key` no `set_scene`/`create_canvas` (`Vec<Arc<str>>` por saída), eliminando o `format!` no rebuild.
Aceite: cena B a 60 fps e cena C com o mouse em movimento: 0 chamadas a `rebuild` por quadro (contador de debug) e 0 alocações por quadro (`dhat`, ou contador em `GlobalAlloc` só em build de debug); CPU do motor reduzida (medir).

**4.2 [Confirmado] Imagens: decodificação no thread de render, sem limite de pixels, sem mips e no tamanho original**
Onde: `E/core/scene2d.rs:219-246` (`grep -n "image::open" E/core/scene2d.rs`); também `E/core/assets/gltf_loader.rs:948`, `E/core/scene3d/environment.rs:46` (estes já rodam fora do loop, mas sem `Limits`).
Causa: `image::open(path)` decodifica tudo (um PNG 20000×20000 pede ~1,6 GB, bomba de descompressão), reduz só se passar de `max_texture_dim` (16384) e sobe sem mipmaps. Com parallax/escala a imagem cintila e a VRAM fica bem acima do necessário.
Correção: `ImageReader::open(p)?.with_guessed_format()?` + `limits.max_alloc = Some(512 MB)` + `max_image_width/height = 16384` (confirmar a API de `Limits` no image 0.25). Alvo = `max(saída) × ui_scale`, arredondado para cima. Gerar mips na GPU com `E/shaders/mipgen.wgsl` (já existe para o 3D) e usar sampler linear com `mipmap_filter: Linear`. Decodificar no loader (1.10).
Aceite: fundo 6000×4000 em monitor 1080p ocupa ≤ 12 MB de VRAM (com mips); PNG de 20k×20k é recusado com aviso em 2.2.

**4.3 [Confirmado] 3D sempre com MSAA 4× em HDR Rgba16F**
Onde: `E/core/scene3d/renderer.rs:277-281` (`Scene3dOptions { msaa: 4 }`), `1047-1050` (alvos); `E/core/governor.rs:211-240` (caps sem MSAA).
Causa: custo fixo grande de VRAM/banda (tabela acima), inclusive em Eco/Ultra.
Correção: `PowerCaps.msaa: u32` (Normal = 4 até 2560×1440 de caixa e 1 acima; Eco = 1; Ultra = 1) e `Scene3dRenderer::set_msaa(n)`, que recria pipelines e alvos só quando o valor muda (pipelines por `sample_count` em cache: `[RenderPipeline; 4]` × {1, 4}). Para a borda sem MSAA, `tonemap.wgsl` já é um passe de tela cheia: adicionar FXAA ali como opção (custo ≈ 1 amostra extra por pixel). Recomendado MSAA por resolução + FXAA em Eco; trade-off: FXAA borra levemente o texto em textura 3D.
Aceite: medir a VRAM dedicada (Process Explorer) na cena C a 1080p e a 4K, antes e depois.

**4.4 [Confirmado] Worker de vídeo acorda a cada 100 ms mesmo sem trabalho**
Onde: `E/platform/windows/video.rs:163-168` (`wait_timeout(guard, WORKER_IDLE_WAIT)`), constante na linha 87.
Correção: `self.cv.wait(guard)` e garantir `cv.notify_one()` em toda transição que o worker espera: slot devolvido em `advance`, `set_paused(false)`, `seek`, `stop`. Auditar com `grep -n "notify" video.rs`.
Aceite: desktop coberto (motor pausado) com vídeo: 0 context switches por segundo na thread `prismview-mf-video` (Process Explorer).

**4.5 [Confirmado] Studio residente faz polling (referência)**
Onde: `T/lib.rs:2409` (visibilidade a cada 500 ms via `compute_monitors_visibility`), `T/lib.rs:2773` (`sleep(250ms)`).
Correção: já descrita em `AUDITORIA.md` 6.3/6.4. Com o wallpaper no motor, a visibilidade só interessa aos widgets: rodar o polling apenas se `!pinned_widgets.is_empty()` e trocar por `SetWinEventHook` (o mesmo padrão de `E/platform/windows/win_events.rs`).
Aceite: sem widgets, a thread `prism-bg-monitor` fica bloqueada (0 ciclos em 60 s).

**4.6 [Verificar] Primeira fonte não encontrada carrega todas as fontes do sistema**
Onde: `E/core/text.rs:257-285` (`resolve_family` → `db.load_system_fonts()`).
Causa: qualquer `font_id` ausente (hoje "Outfit", 2.3) dispara o scan do sistema no thread de render e invalida o shaping de todos os slots.
Como verificar: Private Bytes e tempo do 1º quadro com a cena A antes e depois de 2.3.
Correção: após 2.3 o caso comum some. Para fallback de glifos (emoji, CJK), carregar só `Segoe UI`, `Segoe UI Emoji`, `Segoe UI Symbol` e `Yu Gothic UI` por caminho (`%WINDIR%\Fonts\...`) com `load_font_file`, em vez do sistema inteiro.
Aceite: a cena A sobe sem `load_system_fonts` (log) e com Private Bytes menor (medir).

**4.7 [Verificar] Present FIFO serial em multi-monitor**
Onde: `E/core/gpu.rs:252-253` (`PresentMode::Fifo`, latência 2); `E/native.rs:1009-1046` (laço por saída).
Causa: `get_current_texture` pode bloquear por saída; com 60 Hz + 144 Hz, o loop fica preso ao mais lento e o IPC/sinais esperam.
Como verificar: PIX/GPUView com 2 monitores de taxas diferentes e a cena B.
Correção (se confirmado): `desired_maximum_frame_latency: 1` + `PresentMode::Mailbox` quando suportado (DX12 flip), com fallback `Fifo`; ou renderizar cada saída no seu próprio ritmo (deadline por saída em `about_to_wait`).

---

## Etapa 5 — Estrutural (por último)

**5.1 Arquivos gigantes (> 1500 linhas) misturando responsabilidades**
| Arquivo | Linhas | Divisão proposta |
|---|---|---|
| `S/components/StudioInspector.tsx` | 3846 | `inspector/{TextPanel,GlassPanel,MediaPanel,Scene3DPanel,FxPanel,InteractionPanel}.tsx` + `useInspectorPatch` |
| `T/lib.rs` | 3292 | `projects.rs` (CRUD + atômico), `package.rs` (import/export), `session.rs`, `widgets.rs`, `tray.rs`, `bg_monitor.rs`; `lib.rs` só monta o builder |
| `S/types/prism.ts` | 2639 | `types/` (só tipos), `defaults/`, `templates/`, `normalize/` |
| `S/components/Scene3DLayer.tsx` | 2371 | `scene3d/runtime.ts` (sem React) + componente fino |
| `S/components/StudioEditor.tsx` | 2223 | `editor/{Toolbar,LayerList,Canvas,FilePickers}.tsx` |
| `S/components/UnifiedCompositor.tsx` | 2115 | um arquivo por tipo de camada em `compositor/` |
| `E/core/scene2d.rs` | 1800 | `scene2d/{images,svg,video,frame,touch}.rs` (facilita 4.1) |
| `E/core/glass.rs` | 1575 | `glass/{blur,glyph,groups}.rs` |
| `E/core/scene3d/renderer.rs` | 1516 | `renderer/{targets,pick,draw}.rs` |
Regra: mover código sem mudar comportamento, um arquivo por commit, `cargo check`/`npm run build` entre commits.

**5.2 `prism-core` fora do workspace**
Onde: `Cargo.toml` raiz (members), `CLAUDE.md`. Incluir `crates/prism-core` em `members` para `cargo test -p prism-core` rodar os casos de paridade FX no CI local.

**5.3 Documentação**
Atualizar `CLAUDE.md`: wgpu 24, remoção do wallpaper WebView (2.5), novos comandos/campos (`EngineStatus.errors`, `warnings`), fontes empacotadas (2.3).

---

## Checklist final

- [ ] 0 — Motor e Studio compilam (`cargo check`, `npm run build`)
- [ ] 1 — Saves atômicos + erros visíveis; import sem sobrescrever; OPEN_APP seguro; escopo do asset restrito; instância única do motor e do Studio; pipe local e protegido; loop do motor sem bloqueio; LOAD_SCENE sempre aceito
- [ ] 2 — SHOWCASE no motor; erros do motor na UI; fontes empacotadas; woff2/2ª cena 3D resolvidos; código morto do WebView removido; autosave sem re-zip; sync com aviso; vídeo com erro recuperável; pick vertical
- [ ] 3 — aria-label + foco visível; reduzir movimento; diálogo acessível; chip de salvamento + Ctrl+S; painel de compatibilidade; diálogo pós-import
- [ ] 4 — Medição antes; rebuild 2D só estrutural; imagens com limite + mips; MSAA por caps; worker de vídeo sem polling; Studio sem polling; fontes de fallback; present multi-monitor; medição depois (tabela no PR)
- [ ] 5 — Arquivos divididos; prism-core no workspace; `CLAUDE.md` atualizado
