/** A fixed sequence in [0, 1): the same scene every time it is built. */
export function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    // Numerical Recipes' LCG: plenty for scattering snow.
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

/** `n` rounded to `digits` decimals, so a scene's text stays short. */
export const r = (n: number, digits = 2) => Number(n.toFixed(digits));
