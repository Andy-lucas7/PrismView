/**
 * Orçamento de miniaturas "vivas" do Hub. Cada miniatura viva é um UnifiedCompositor completo
 * (até 1 WebGL); o Chromium limita ~16 contextos. Aqui só as `capacity` melhores (menor `rank`)
 * entre as que pedem ficam vivas; o resto é desmontado (dispose via unmount).
 * Puro (sem React/DOM) para ser testável.
 */
export type LiveListener = () => void;

export class HubLiveBudget {
  private readonly wants = new Map<string, { rank: number; seq: number }>();
  private live = new Set<string>();
  private readonly listeners = new Set<LiveListener>();
  private seq = 0;
  private pending = false;

  readonly capacity: number;

  constructor(capacity: number) {
    this.capacity = capacity;
  }

  /** `rank` menor = mais prioritário (0 = centro/totalmente visível). `null` = não quer mais. */
  request(id: string, rank: number | null): void {
    if (rank === null) {
      if (!this.wants.delete(id)) return;
    } else {
      const cur = this.wants.get(id);
      if (cur && cur.rank === rank) return;
      this.wants.set(id, { rank, seq: cur ? cur.seq : this.seq++ });
    }
    this.schedule();
  }

  isLive(id: string): boolean {
    return this.live.has(id);
  }

  subscribe(fn: LiveListener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** Reavalia agora (testes) e notifica só se o conjunto vivo mudou. */
  flush(): void {
    this.pending = false;
    const next = new Set(
      [...this.wants.entries()]
        .sort((a, b) => a[1].rank - b[1].rank || a[1].seq - b[1].seq)
        .slice(0, this.capacity)
        .map(([id]) => id)
    );
    // Histerese: quem já está vivo e empata com o corte não é trocado (evita pisca).
    if (next.size === this.live.size && [...next].every((id) => this.live.has(id))) return;
    this.live = next;
    for (const fn of [...this.listeners]) fn();
  }

  private schedule(): void {
    if (this.pending) return;
    this.pending = true;
    queueMicrotask(() => this.pending && this.flush());
  }
}
