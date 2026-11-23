/**
 * Traduz erros crus do backend (Rust/IPC/Win32) para avisos curtos ao usuário.
 * O detalhe técnico vai só para `console.error`.
 */

const RULES: Array<[RegExp, string]> = [
  [/invalid_json|invalid_response/i, "O motor respondeu de forma inesperada. Reinicie o PrismView e tente de novo."],
  [/engine_timeout|engine_start_timeout/i, "O motor nativo demorou a responder. Tente novamente em instantes."],
  [/engine_busy/i, "O motor nativo está ocupado carregando a cena. Tente novamente em instantes."],
  [/scene_path_denied/i, "O motor recusou um arquivo de cena fora da pasta do PrismView."],
  [/engine_closed|pipe_closed|engine_unreachable|pipe_write|pipe_read/i, "Não foi possível falar com o motor nativo. Ele pode ter sido encerrado."],
  [/engine_exited_early|engine_spawn_failed|PRISMVIEW_ENGINE_EXE/i, "O motor nativo não conseguiu iniciar. Verifique a instalação do PrismView."],
  [/gpu_unavailable/i, "A GPU não está disponível no momento. Tente novamente."],
  [/scene_invalid|scene_file_too_large/i, "A cena deste projeto não pôde ser carregada pelo motor."],
  [/scene_file|native_scene|native_state_dir|power_preference_write/i, "Não foi possível salvar o estado do wallpaper em disco."],
  [/invalid_power_preference/i, "Modo de energia inválido."],
  [/native_mode_unsupported_command/i, "Este recurso não está disponível no motor nativo."],
  [/os error (5|1314)\b|access is denied|acesso negado/i, "Sem permissão para acessar o arquivo ou a pasta."],
  [/os error (2|3)\b|cannot find|not found|não foi possível encontrar|nao foi possivel encontrar/i, "Arquivo não encontrado. Ele pode ter sido movido ou apagado."],
  [/os error (32|33)\b|being used by another process/i, "O arquivo está em uso por outro programa."],
  [/os error 112\b|no space left|espaço insuficiente/i, "Sem espaço em disco."],
  [/os error \d+/i, "Falha ao acessar um arquivo do sistema."],
  [/zip|archive|\.prism/i, "O pacote .prism está inválido ou corrompido."],
  [/Tauri offline/i, "Disponível apenas no app desktop."],
];

export function friendlyError(err: unknown, fallback = "Algo deu errado. Tente novamente."): string {
  console.error("[PrismView]", err);
  const raw =
    typeof err === "string"
      ? err
      : err instanceof Error
        ? err.message
        : (() => {
            try {
              return JSON.stringify(err);
            } catch {
              return "";
            }
          })();
  for (const [re, msg] of RULES) {
    if (re.test(raw)) return msg;
  }
  return fallback;
}
