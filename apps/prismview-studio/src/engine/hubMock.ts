/**
 * Valores fixos das miniaturas do Hub: hora, data, clima, bateria e mídia nunca mudam,
 * então o card não depende de relógio/telemetria e fica idêntico entre aberturas.
 */
import type { LiveTelemetry } from "../types/prism";
import type { FxWeather } from "./fx/fxEngine";

/** Quinta-feira, 12/03/2026, 18:45:00 (hora local). */
export const HUB_MOCK_NOW = new Date(2026, 2, 12, 18, 45, 0, 0);

export const HUB_MOCK_WEATHER: FxWeather = {
  temp_c: 24,
  condition: "Parcialmente nublado",
  code: 2,
  city: "São Paulo",
  is_day: true,
};

export const HUB_MOCK_TELEMETRY: LiveTelemetry = {
  cpu_percent: 34,
  ram_percent: 48,
  ram_used_gb: 15.4,
  ram_total_gb: 32,
  hostname: "PrismView",
  battery_percent: 82,
  battery_charging: false,
  media: {
    title: "Midnight City",
    artist: "M83",
    album: "Hurry Up, We're Dreaming",
    playing: true,
    cover_path: "",
    position_secs: 84,
    duration_secs: 243,
    updated_at_ms: HUB_MOCK_NOW.getTime(),
  },
};
