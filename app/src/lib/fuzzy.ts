// Tiny subsequence matcher. Returns null when the query is not a subsequence of
// the text; otherwise a score that rewards contiguous runs and word starts.

export function fuzzyScore(query: string, text: string): number | null {
  const q = query.trim().toLowerCase();
  if (!q) return 0;
  const t = text.toLowerCase();
  // Exact substring is always the strongest signal.
  const direct = t.indexOf(q);
  if (direct >= 0) return 100 - direct * 0.5 + (isBoundary(t, direct) ? 20 : 0);

  let score = 0;
  let ti = 0;
  let run = 0;
  for (const ch of q) {
    if (ch === ' ') continue;
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    run = found === ti ? run + 1 : 0;
    score += 1 + run * 2 + (isBoundary(t, found) ? 4 : 0) - Math.min(3, (found - ti) * 0.1);
    ti = found + 1;
  }
  return score;
}

function isBoundary(text: string, index: number): boolean {
  if (index === 0) return true;
  const prev = text[index - 1] ?? ' ';
  return /[\s\-_/.:#@]/.test(prev);
}

/** Filter by fuzzy match, keep caller's ordering (recency) among matches. */
export function fuzzyFilter<T>(items: T[], query: string, key: (item: T) => string): T[] {
  if (!query.trim()) return items;
  return items.filter((item) => fuzzyScore(query, key(item)) !== null);
}

/** Ranked variant for things without a natural order (e.g. actions). */
export function fuzzyRank<T>(items: T[], query: string, key: (item: T) => string): T[] {
  if (!query.trim()) return items;
  return items
    .map((item) => ({ item, score: fuzzyScore(query, key(item)) }))
    .filter((x): x is { item: T; score: number } => x.score !== null)
    .sort((a, b) => b.score - a.score)
    .map((x) => x.item);
}
