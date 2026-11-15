import { r, seeded } from "./random";

/*
 * Frost Ferns: frost grows in from the corners of the glass in feathered
 * fronds, breathes a little, and glints at its tips; the middle stays clear.
 * The fronds are drawn once — each corner breathes as one layer, by transform
 * and opacity — and a faint sheen crosses the glass now and then.
 */

/** One frond from (x, y) at `angle`: a bending stem with smaller fronds off alternate sides. */
function frond(rand: () => number, x: number, y: number, angle: number, length: number, depth: number): string {
  const steps = 5;
  const step = length / steps;
  const bend = (rand() - 0.5) * 0.5;
  let d = `M${r(x, 1)} ${r(y, 1)}`;
  const joints: [number, number, number][] = [];
  for (let i = 0; i < steps; i++) {
    angle += bend / steps;
    x += Math.cos(angle) * step;
    y += Math.sin(angle) * step;
    d += `L${r(x, 1)} ${r(y, 1)}`;
    joints.push([x, y, angle]);
  }
  if (depth === 0) return d;
  return joints
    .slice(0, -1)
    .reduce(
      (all, [jx, jy, ja], i) =>
        `${all}${frond(rand, jx, jy, ja + (i % 2 ? 1 : -1) * (0.85 + rand() * 0.35), step * (steps - i) * 0.42, depth - 1)}`,
      d,
    );
}

/** A corner's fronds, in a 100×100 box whose (0, 0) is the corner. */
function corner(seed: number): string {
  const rand = seeded(seed);
  const stems = [0.3, 0.78, 1.25].map((angle) => frond(rand, 0, 0, angle + (rand() - 0.5) * 0.2, 62 + rand() * 22, 2));
  return `<svg viewBox="0 0 100 100" aria-hidden="true"><path d="${stems.join("")}"/></svg>`;
}

const GLINTS: [string, number, number][] = [
  ["left:21vmin;top:9vmin", 9, 1],
  ["left:8vmin;top:30vmin", 12, 6],
  ["right:24vmin;top:7vmin", 10, 3],
  ["right:10vmin;top:26vmin", 13, 9],
  ["left:18vmin;bottom:11vmin", 11, 4],
  ["left:33vmin;bottom:5vmin", 8, 7],
  ["right:16vmin;bottom:14vmin", 12, 2],
  ["right:31vmin;bottom:6vmin", 9, 5],
];

export function frostFerns(): string {
  const corners = (["tl", "tr", "bl", "br"] as const)
    .map((at, i) => `<div class="corner ${at}"><div class="grow" style="animation-delay:-${i * 9}s">${corner(11 + i * 17)}</div></div>`)
    .join("");
  const glints = GLINTS.map(
    ([at, duration, delay]) => `<i class="glint" style="${at};animation-duration:${duration}s;animation-delay:-${delay}s"></i>`,
  ).join("");
  return `<style>
body{background:radial-gradient(ellipse at 50% 55%,#0b1424 0%,#070c16 75%)}
.corner{position:absolute;width:48vmin;height:48vmin}
.corner.tl{left:0;top:0}
.corner.tr{right:0;top:0;transform:scaleX(-1)}
.corner.bl{left:0;bottom:0;transform:scaleY(-1)}
.corner.br{right:0;bottom:0;transform:scale(-1,-1)}
.grow{width:100%;height:100%;transform-origin:0 0;animation:grow 38s ease-in-out infinite alternate}
.grow svg{display:block;width:100%;height:100%}
.grow path{fill:none;stroke:rgba(205,232,255,.42);stroke-width:.42;stroke-linecap:round;stroke-linejoin:round}
.glint{position:absolute;width:1.6vmin;height:1.6vmin;border-radius:50%;background:radial-gradient(closest-side,#fff,rgba(200,230,255,.5) 40%,rgba(200,230,255,0));opacity:0;animation:glint ease-in-out infinite}
.sheen{position:absolute;top:0;left:-100vw;width:100vw;height:100vh;background:linear-gradient(105deg,rgba(200,230,255,0) 35%,rgba(200,230,255,.045) 50%,rgba(200,230,255,0) 65%);animation:sheen 46s ease-in-out infinite}
@keyframes grow{from{transform:scale(.93);opacity:.6}to{transform:scale(1);opacity:1}}
@keyframes glint{0%,70%,100%{opacity:0;transform:scale(.4)}82%{opacity:.9;transform:scale(1)}}
@keyframes sheen{0%,55%{transform:translateX(0)}100%{transform:translateX(200vw)}}
</style>
${corners}${glints}<div class="sheen"></div>`;
}
