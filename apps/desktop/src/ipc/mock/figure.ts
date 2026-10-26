// Inline SVG figure generation for mock runs (deterministic per seed).

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function gauss(rng: () => number): number {
  const u = Math.max(rng(), 1e-9);
  const v = rng();
  return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v);
}

export interface SimParams {
  n: number;
  tau: number;
  amplitude: number;
  noise: number;
  seed: number;
}

export function simParamsFromArgs(args: string[]): SimParams {
  const p: SimParams = { n: 200, tau: 2.5, amplitude: 1.0, noise: 0.05, seed: 0 };
  for (let i = 0; i < args.length; i++) {
    const a = args[i] ?? "";
    const next = args[i + 1];
    const num = next !== undefined ? Number(next) : NaN;
    if (!Number.isFinite(num)) continue;
    if (a === "--n") p.n = Math.max(5, Math.min(2000, Math.round(num)));
    else if (a === "--tau") p.tau = Math.max(0.05, num);
    else if (a === "--amplitude") p.amplitude = num;
    else if (a === "--noise") p.noise = Math.max(0, num);
    else if (a === "--seed") p.seed = Math.round(num);
  }
  return p;
}

const W = 640;
const H = 440;
const ML = 60;
const MR = 20;
const MT = 40;
const MB = 50;

function axes(title: string, xlabel: string, ylabel: string, yTicks: string[], xTicks: string[]): string {
  const pw = W - ML - MR;
  const ph = H - MT - MB;
  const ticksY = yTicks
    .map((t, i) => {
      const y = MT + ph - (i / (yTicks.length - 1)) * ph;
      return `<text x="${ML - 8}" y="${y + 4}" text-anchor="end" font-size="11" fill="#444">${t}</text><line x1="${ML}" y1="${y}" x2="${ML + pw}" y2="${y}" stroke="#eee"/>`;
    })
    .join("");
  const ticksX = xTicks
    .map((t, i) => {
      const x = ML + (i / (xTicks.length - 1)) * pw;
      return `<text x="${x}" y="${MT + ph + 18}" text-anchor="middle" font-size="11" fill="#444">${t}</text>`;
    })
    .join("");
  return `<rect x="0" y="0" width="${W}" height="${H}" fill="white"/>
<text x="${W / 2}" y="24" text-anchor="middle" font-size="15" font-family="sans-serif" fill="#222">${title}</text>
${ticksY}${ticksX}
<rect x="${ML}" y="${MT}" width="${pw}" height="${ph}" fill="none" stroke="#333"/>
<text x="${ML + pw / 2}" y="${H - 12}" text-anchor="middle" font-size="12" fill="#222">${xlabel}</text>
<text x="16" y="${MT + ph / 2}" text-anchor="middle" font-size="12" fill="#222" transform="rotate(-90 16 ${MT + ph / 2})">${ylabel}</text>`;
}

export function decaySvg(p: SimParams): { svg: string; tauFit: number; rms: number } {
  const rng = mulberry32(p.seed + 1);
  const pw = W - ML - MR;
  const ph = H - MT - MB;
  const tmax = 5 * p.tau;
  const pts: [number, number][] = [];
  let sq = 0;
  for (let i = 0; i < p.n; i++) {
    const t = (i / (p.n - 1)) * tmax;
    const clean = p.amplitude * Math.exp(-t / p.tau);
    const e = gauss(rng) * p.noise;
    sq += e * e;
    pts.push([t, clean + e]);
  }
  const ymax = Math.max(p.amplitude * 1.15, 0.1);
  const ymin = Math.min(-0.15 * Math.abs(p.amplitude), -0.05);
  const sx = (t: number) => ML + (t / tmax) * pw;
  const sy = (y: number) => MT + ph - ((y - ymin) / (ymax - ymin)) * ph;
  const circles = pts
    .map(([t, y]) => `<circle cx="${sx(t).toFixed(1)}" cy="${sy(y).toFixed(1)}" r="2" fill="#1f77b4" opacity="0.8"/>`)
    .join("");
  const tauFit = p.tau * (1 + (rng() - 0.5) * 0.03);
  const fit: string[] = [];
  for (let i = 0; i <= 100; i++) {
    const t = (i / 100) * tmax;
    fit.push(`${sx(t).toFixed(1)},${sy(p.amplitude * Math.exp(-t / tauFit)).toFixed(1)}`);
  }
  const yTicks = [ymin, (ymin + ymax) / 2, ymax].map((v) => v.toFixed(2));
  const xTicks = [0, tmax / 2, tmax].map((v) => v.toFixed(1));
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" font-family="sans-serif">
${axes("Decay simulation", "t", "y", yTicks, xTicks)}
${circles}
<polyline points="${fit.join(" ")}" fill="none" stroke="#ff7f0e" stroke-width="2"/>
<rect x="${W - MR - 110}" y="${MT + 10}" width="100" height="40" fill="white" stroke="#999"/>
<circle cx="${W - MR - 98}" cy="${MT + 24}" r="3" fill="#1f77b4"/><text x="${W - MR - 88}" y="${MT + 28}" font-size="11">data</text>
<line x1="${W - MR - 103}" y1="${MT + 40}" x2="${W - MR - 93}" y2="${MT + 40}" stroke="#ff7f0e" stroke-width="2"/><text x="${W - MR - 88}" y="${MT + 44}" font-size="11">fit</text>
</svg>`;
  return { svg, tauFit, rms: Math.sqrt(sq / p.n) };
}

export function residualsSvg(p: SimParams): string {
  const rng = mulberry32(p.seed + 7);
  const pw = W - ML - MR;
  const ph = H - MT - MB;
  const tmax = 5 * p.tau;
  const lim = Math.max(p.noise * 3.5, 0.01);
  const sx = (t: number) => ML + (t / tmax) * pw;
  const sy = (y: number) => MT + ph - ((y + lim) / (2 * lim)) * ph;
  const circles: string[] = [];
  for (let i = 0; i < p.n; i++) {
    const t = (i / (p.n - 1)) * tmax;
    circles.push(`<circle cx="${sx(t).toFixed(1)}" cy="${sy(gauss(rng) * p.noise).toFixed(1)}" r="2" fill="#2ca02c" opacity="0.8"/>`);
  }
  const yTicks = [-lim, 0, lim].map((v) => v.toFixed(2));
  const xTicks = [0, tmax / 2, tmax].map((v) => v.toFixed(1));
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" font-family="sans-serif">
${axes("Residuals", "t", "y - fit", yTicks, xTicks)}
<line x1="${ML}" y1="${sy(0)}" x2="${ML + pw}" y2="${sy(0)}" stroke="#999" stroke-dasharray="4 3"/>
${circles.join("")}
</svg>`;
}
