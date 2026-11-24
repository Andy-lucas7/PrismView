/**
 * Cache de "stills" (quadro parado) de camadas VIDEO/SCENE_3D para as miniaturas do Hub.
 * Miniaturas fora do nível `full` mostram a imagem em vez de manter decoder de vídeo / contexto
 * WebGL vivo. LRU com revoke das URLs. Sem DOM aqui (captura fica em hubStillCapture.ts).
 */
export class StillCache {
  private readonly map = new Map<string, string>();
  private readonly listeners = new Set<() => void>();
  private ver = 0;
  readonly capacity: number;
  private readonly revoke: (url: string) => void;

  constructor(capacity: number, revoke: (url: string) => void = () => {}) {
    this.capacity = capacity;
    this.revoke = revoke;
  }

  static key(projectId: string, layerId: string, sig: string): string {
    return `${projectId}|${layerId}|${sig}`;
  }

  get(key: string): string | undefined {
    const url = this.map.get(key);
    if (url !== undefined) {
      this.map.delete(key);
      this.map.set(key, url);
    }
    return url;
  }

  /** Lê sem tocar na ordem LRU (seguro para `getSnapshot`). */
  peek(key: string): string | undefined {
    return this.map.get(key);
  }

  has(key: string): boolean {
    return this.map.has(key);
  }

  set(key: string, url: string): void {
    const old = this.map.get(key);
    if (old !== undefined) {
      this.map.delete(key);
      if (old !== url) this.revoke(old);
    }
    this.map.set(key, url);
    while (this.map.size > this.capacity) {
      const first = this.map.keys().next().value as string;
      const u = this.map.get(first) as string;
      this.map.delete(first);
      this.revoke(u);
    }
    this.ver++;
    for (const fn of [...this.listeners]) fn();
  }

  version(): number {
    return this.ver;
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
}
