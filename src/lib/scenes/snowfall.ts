import { r, seeded } from "./random";

/*
 * Snowfall: snow over a frozen lake and far hills at night. The flakes are
 * large and soft and thicker at the sides; the middle, where the conversation
 * sits, gets the fewest. Only the flakes, a moon's glow and a sheen on the ice
 * move — by transform and opacity, which the compositor animates without
 * drawing the scene again.
 */
export function snowfall(): string {
  const rand = seeded(7);
  let flakes = "";
  for (let i = 0; i < 64; i++) {
    let x = rand();
    // Two of three flakes that would fall down the middle third fall at a side instead.
    if (x > 0.33 && x < 0.67 && rand() < 0.66) x = x < 0.5 ? x - 0.3 : x + 0.3;
    const size = r(0.7 + rand() * 1.4);
    const duration = r(24 + rand() * 26, 1);
    flakes +=
      `<i class="f" style="left:${r(x * 100, 1)}%;width:${size}vmin;height:${size}vmin;opacity:${r(0.4 + rand() * 0.5)};` +
      `animation:sway-${"abc"[i % 3]} ${duration}s linear ${r(-rand() * duration, 1)}s infinite"></i>`;
  }
  return `<style>
body{background:linear-gradient(180deg,#070b1a 0%,#0d1530 45%,#1a2442 70%,#22304f 78%,#0e1628 100%)}
.glow{position:absolute;right:-12vw;top:-18vh;width:60vw;height:60vh;background:radial-gradient(closest-side,rgba(170,200,255,.2),rgba(170,200,255,0));animation:breathe 34s ease-in-out infinite alternate}
.hills{position:absolute;left:0;bottom:12vh;width:100%;height:30vh}
.lake{position:absolute;left:0;bottom:0;width:100%;height:12.5vh;background:linear-gradient(180deg,#2a3c5e,#0e1a2e)}
.sheen{position:absolute;left:10vw;bottom:6vh;width:44vw;height:2.4vh;background:radial-gradient(closest-side,rgba(200,225,255,.13),rgba(200,225,255,0));animation:glide 58s ease-in-out infinite alternate}
.f{position:absolute;top:-4vmin;border-radius:50%;background:radial-gradient(closest-side,#fff,rgba(255,255,255,.55) 45%,rgba(255,255,255,0))}
@keyframes sway-a{0%{transform:translate3d(0,0,0)}25%{transform:translate3d(1.6vw,28vh,0)}50%{transform:translate3d(0,56vh,0)}75%{transform:translate3d(-1.6vw,84vh,0)}100%{transform:translate3d(0,112vh,0)}}
@keyframes sway-b{0%{transform:translate3d(0,0,0)}25%{transform:translate3d(-2.2vw,28vh,0)}50%{transform:translate3d(-.6vw,56vh,0)}75%{transform:translate3d(1.4vw,84vh,0)}100%{transform:translate3d(.4vw,112vh,0)}}
@keyframes sway-c{0%{transform:translate3d(0,0,0)}25%{transform:translate3d(1vw,28vh,0)}50%{transform:translate3d(2.4vw,56vh,0)}75%{transform:translate3d(1vw,84vh,0)}100%{transform:translate3d(-.8vw,112vh,0)}}
@keyframes breathe{from{opacity:.55}to{opacity:1}}
@keyframes glide{from{transform:translateX(-6vw)}to{transform:translateX(30vw)}}
</style>
<div class="glow"></div>
<svg class="hills" viewBox="0 0 100 30" preserveAspectRatio="none" aria-hidden="true">
<path d="M0 30V14C8 10 14 7 22 9S36 15 45 11 60 4 70 7 86 13 100 9V30Z" fill="#26375a"/>
<path d="M0 30V21C10 18 18 16 28 19S44 24 55 20 72 15 82 18 94 22 100 20V30Z" fill="#1a2843"/>
</svg>
<div class="lake"></div>
<div class="sheen"></div>
${flakes}`;
}
