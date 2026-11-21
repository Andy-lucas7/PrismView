import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 2.5: project state is immutable (updates are spread-patches), so snapshots are stored by
 * reference: no structuredClone per step and no JSON.stringify comparison.
 */
function cloneSnapshot<T>(value: T): T {
  return value;
}

function snapshotsEqual<T>(a: T, b: T): boolean {
  return Object.is(a, b);
}

export interface UseHistoryOptions {
  /** Maximum number of undo states kept in memory */
  limit?: number;
  /** Reset history stack whenever this identity changes (e.g. project.id) */
  resetKey: string;
}

export interface UseHistoryApi<T> {
  canUndo: boolean;
  canRedo: boolean;
  /** Records a discrete pre-change snapshot onto the undo stack */
  pushSnapshot: (beforeState: T) => void;
  /** Starts a batched interaction (drag/resize/slider); ignores intermediate frames */
  beginTransaction: (initialState: T) => void;
  /** Commits the transaction only if finalState differs from the initialState */
  commitTransaction: (finalState: T) => void;
  /** Cancels an active transaction without pushing to history */
  cancelTransaction: () => void;
  /** Records rapid repeated edits (e.g. keyboard nudge) as a single coalesced step */
  recordCoalesced: (beforeState: T, windowMs?: number) => void;
  /** Restores previous state and pushes currentState onto redo stack */
  undo: (currentState: T) => T | null;
  /** Restores next state and pushes currentState onto undo stack */
  redo: (currentState: T) => T | null;
}

export function useHistory<T>({ limit = 50, resetKey }: UseHistoryOptions): UseHistoryApi<T> {
  const pastRef = useRef<T[]>([]);
  const futureRef = useRef<T[]>([]);
  const txInitialRef = useRef<T | null>(null);
  const coalesceTimerRef = useRef<number | null>(null);
  const coalesceOpenRef = useRef(false);

  const [counts, setCounts] = useState<{ past: number; future: number }>({
    past: 0,
    future: 0,
  });

  const syncCounts = useCallback(() => {
    setCounts({ past: pastRef.current.length, future: futureRef.current.length });
  }, []);

  useEffect(() => {
    pastRef.current = [];
    futureRef.current = [];
    txInitialRef.current = null;
    coalesceOpenRef.current = false;
    if (coalesceTimerRef.current !== null) {
      window.clearTimeout(coalesceTimerRef.current);
      coalesceTimerRef.current = null;
    }
    syncCounts();
  }, [resetKey, syncCounts]);

  useEffect(() => {
    return () => {
      if (coalesceTimerRef.current !== null) {
        window.clearTimeout(coalesceTimerRef.current);
      }
    };
  }, []);

  const pushPast = useCallback(
    (snapshot: T) => {
      const cloned = cloneSnapshot(snapshot);
      const nextPast = [...pastRef.current, cloned];
      pastRef.current = nextPast.length > limit ? nextPast.slice(nextPast.length - limit) : nextPast;
      futureRef.current = [];
      syncCounts();
    },
    [limit, syncCounts]
  );

  const pushSnapshot = useCallback(
    (beforeState: T) => {
      coalesceOpenRef.current = false;
      if (coalesceTimerRef.current !== null) {
        window.clearTimeout(coalesceTimerRef.current);
        coalesceTimerRef.current = null;
      }
      pushPast(beforeState);
    },
    [pushPast]
  );

  const beginTransaction = useCallback((initialState: T) => {
    if (txInitialRef.current === null) {
      txInitialRef.current = cloneSnapshot(initialState);
    }
  }, []);

  const commitTransaction = useCallback(
    (finalState: T) => {
      const initial = txInitialRef.current;
      txInitialRef.current = null;
      if (initial === null) return;
      if (!snapshotsEqual(initial, finalState)) {
        pushPast(initial);
      }
    },
    [pushPast]
  );

  const cancelTransaction = useCallback(() => {
    txInitialRef.current = null;
  }, []);

  const recordCoalesced = useCallback(
    (beforeState: T, windowMs = 350) => {
      if (!coalesceOpenRef.current) {
        coalesceOpenRef.current = true;
        pushPast(beforeState);
      }
      if (coalesceTimerRef.current !== null) {
        window.clearTimeout(coalesceTimerRef.current);
      }
      coalesceTimerRef.current = window.setTimeout(() => {
        coalesceOpenRef.current = false;
        coalesceTimerRef.current = null;
      }, windowMs);
    },
    [pushPast]
  );

  const undo = useCallback(
    (currentState: T): T | null => {
      txInitialRef.current = null;
      coalesceOpenRef.current = false;
      if (pastRef.current.length === 0) return null;
      const previous = pastRef.current[pastRef.current.length - 1];
      pastRef.current = pastRef.current.slice(0, -1);
      const nextFuture = [...futureRef.current, cloneSnapshot(currentState)];
      futureRef.current =
        nextFuture.length > limit ? nextFuture.slice(nextFuture.length - limit) : nextFuture;
      syncCounts();
      return cloneSnapshot(previous);
    },
    [limit, syncCounts]
  );

  const redo = useCallback(
    (currentState: T): T | null => {
      txInitialRef.current = null;
      coalesceOpenRef.current = false;
      if (futureRef.current.length === 0) return null;
      const next = futureRef.current[futureRef.current.length - 1];
      futureRef.current = futureRef.current.slice(0, -1);
      const nextPast = [...pastRef.current, cloneSnapshot(currentState)];
      pastRef.current = nextPast.length > limit ? nextPast.slice(nextPast.length - limit) : nextPast;
      syncCounts();
      return cloneSnapshot(next);
    },
    [limit, syncCounts]
  );

  return {
    canUndo: counts.past > 0,
    canRedo: counts.future > 0,
    pushSnapshot,
    beginTransaction,
    commitTransaction,
    cancelTransaction,
    recordCoalesced,
    undo,
    redo,
  };
}
