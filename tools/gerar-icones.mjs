// Gera src/icons.rs com os caminhos SVG dos ícones do Lucide que o painel usa.
// Uso: node tools/gerar-icones.mjs <pasta do pacote lucide>
import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

const NAMES = [
  'air-vent', 'arrow-left-right', 'check', 'chevron-left', 'chevron-right', 'chevron-up', 'cloud-off', 'clock',
  'droplet', 'droplets', 'external-link', 'fan', 'feather', 'flame', 'loader-circle', 'lock', 'log-in', 'log-out',
  'minus', 'monitor', 'moon', 'move', 'move-horizontal', 'move-vertical', 'plus', 'power', 'refresh-cw', 'rocket', 'search',
  'shield-check', 'signal', 'signal-high', 'signal-low', 'signal-medium', 'sliders-horizontal', 'snowflake',
  'sparkles', 'sun', 'thermometer', 'triangle-alert', 'volume-2', 'wind', 'x', 'zap',
];

const packageDir = path.resolve(process.argv[2] ?? 'node_modules/lucide');
const lucide = createRequire(import.meta.url)(path.join(packageDir, 'dist/umd/lucide.js'));
const { version } = JSON.parse(readFileSync(path.join(packageDir, 'package.json'), 'utf8'));
const pascal = (name) => name.replace(/(^|-)(\w)/g, (_match, _dash, letter) => letter.toUpperCase());
const n = (value) => Number(value);

function ellipse(cx, cy, rx, ry) {
  return `M${cx - rx} ${cy}a${rx} ${ry} 0 1 0 ${2 * rx} 0a${rx} ${ry} 0 1 0 ${-2 * rx} 0Z`;
}

function rect({ x = 0, y = 0, width, height, rx, ry }) {
  const [left, top, w, h] = [n(x), n(y), n(width), n(height)];
  const r = Math.min(n(rx ?? ry ?? 0), w / 2, h / 2);
  if (!r) return `M${left} ${top}h${w}v${h}h${-w}Z`;
  return (
    `M${left + r} ${top}h${w - 2 * r}a${r} ${r} 0 0 1 ${r} ${r}v${h - 2 * r}a${r} ${r} 0 0 1 ${-r} ${r}` +
    `h${-(w - 2 * r)}a${r} ${r} 0 0 1 ${-r} ${-r}v${-(h - 2 * r)}a${r} ${r} 0 0 1 ${r} ${-r}Z`
  );
}

function toPath([tag, attrs]) {
  switch (tag) {
    case 'path':
      return attrs.d;
    case 'circle':
      return ellipse(n(attrs.cx), n(attrs.cy), n(attrs.r), n(attrs.r));
    case 'ellipse':
      return ellipse(n(attrs.cx), n(attrs.cy), n(attrs.rx), n(attrs.ry));
    case 'rect':
      return rect(attrs);
    case 'line':
      return `M${attrs.x1} ${attrs.y1}L${attrs.x2} ${attrs.y2}`;
    case 'polyline':
    case 'polygon': {
      const points = attrs.points.trim().split(/[\s,]+/);
      const pairs = [];
      for (let i = 0; i < points.length; i += 2) pairs.push(`${points[i]} ${points[i + 1]}`);
      return `M${pairs.join('L')}${tag === 'polygon' ? 'Z' : ''}`;
    }
    default:
      throw new Error(`Elemento SVG sem conversão: ${tag}`);
  }
}

const entries = NAMES.map((name) => {
  const node = lucide.icons[pascal(name)];
  if (!node) throw new Error(`Ícone não encontrado: ${name}`);
  const paths = node.map(toPath).map((d) => JSON.stringify(d));
  return `    ("${name}", &[${paths.join(', ')}]),`;
});

const source = `// Gerado por tools/gerar-icones.mjs a partir do Lucide ${version} (ISC, https://lucide.dev).
// Cada ícone é desenhado num quadro de 24 por 24 com traço arredondado.

pub const ICONS: &[(&str, &[&str])] = &[
${entries.join('\n')}
];
`;
writeFileSync(new URL('../src/icons.rs', import.meta.url), source);
console.log(`src/icons.rs com ${NAMES.length} ícones (${Buffer.byteLength(source)} bytes)`);
