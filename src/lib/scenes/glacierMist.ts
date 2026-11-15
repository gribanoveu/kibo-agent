/*
 * Glacier Mist: three ridges of blue glacier ice along the bottom, mist
 * drifting slowly between them, a cold light in the upper left. The ridges are
 * drawn once; only the mist and the light move, by transform and opacity.
 */
const ridge = (id: string, d: string, top: string, bottom: string) =>
  `<svg class="ridge ${id}" viewBox="0 0 100 40" preserveAspectRatio="none" aria-hidden="true">` +
  `<defs><linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${top}"/><stop offset="1" stop-color="${bottom}"/></linearGradient></defs>` +
  `<path d="${d}" fill="url(#${id})"/></svg>`;

const mist = (bottom: number, left: number, duration: number, delay: number) =>
  `<div class="mist" style="bottom:${bottom}vh;left:${left}vw;animation-duration:${duration}s;animation-delay:-${delay}s"></div>`;

export function glacierMist(): string {
  return `<style>
body{background:linear-gradient(180deg,#050b14 0%,#0a1826 50%,#10283a 100%)}
.light{position:absolute;left:-14vw;top:-22vh;width:62vw;height:66vh;background:radial-gradient(closest-side,rgba(140,200,235,.16),rgba(140,200,235,0));animation:breathe 40s ease-in-out infinite alternate}
.ridge{position:absolute;left:0;bottom:0;width:100%}
.ridge.far{height:46vh}
.ridge.mid{height:34vh}
.ridge.near{height:22vh}
.mist{position:absolute;width:84vw;height:17vh;background:radial-gradient(closest-side,rgba(190,222,240,.15),rgba(190,222,240,0));animation:drift ease-in-out infinite alternate}
@keyframes drift{from{transform:translateX(-9vw)}to{transform:translateX(9vw)}}
@keyframes breathe{from{opacity:.5}to{opacity:1}}
</style>
<div class="light"></div>
${ridge("far", "M0 40V20L9 13 16 17 27 7 36 15 44 11 55 3 63 12 72 9 83 16 92 10 100 14V40Z", "#4c7189", "#0f2132")}
${mist(30, -10, 96, 20)}${mist(27, 40, 112, 70)}
${ridge("mid", "M0 40V22L12 15 21 20 33 10 42 18 52 14 61 21 74 12 86 19 100 13V40Z", "#3a5a72", "#0b1a28")}
${mist(15, 55, 88, 10)}${mist(12, -20, 104, 50)}
${ridge("near", "M0 40V24L10 18 24 25 36 17 50 26 64 19 78 26 90 18 100 23V40Z", "#2b465b", "#081420")}
${mist(-4, 20, 120, 35)}`;
}
