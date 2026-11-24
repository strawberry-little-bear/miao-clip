const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const stylesheet = fs.readFileSync(path.join(__dirname, '..', 'ui', 'style.css'), 'utf8');
const rules = [...stylesheet.matchAll(/([^{}]+)\{([^{}]*)\}/g)];

function declarationsFor(selector) {
  const rule = rules.find(([, selectors]) => selectors
    .split(',')
    .some((candidate) => candidate.trim() === selector));
  assert.ok(rule, `Missing CSS rule for ${selector}`);
  return rule[2];
}

test('splitter stays as a subtle neutral line while idle', () => {
  const line = declarationsFor('.layout-splitter::after');
  const handle = declarationsFor('.layout-splitter::before');

  assert.match(line, /background:\s*#e6e4de\b/);
  assert.match(handle, /opacity:\s*0(?:;|\s|$)/);
  assert.doesNotMatch(`${line}\n${handle}`, /box-shadow:\s*(?!none\b)[^;]+/i);
});

test('splitter only reveals a restrained neutral grip during interaction', () => {
  const activeLine = declarationsFor('.layout-splitter:hover::after');
  const activeHandle = declarationsFor('.layout-splitter:hover::before');

  assert.match(activeLine, /background:\s*#d8d5cc\b/);
  assert.match(activeHandle, /border-color:\s*#b1ada3\b/);
  assert.match(activeHandle, /opacity:\s*0\.54\b/);
});

test('keyboard focus remains visible without a blue glow', () => {
  const focus = declarationsFor('.layout-splitter:focus-visible');

  assert.match(focus, /outline:\s*1px\s+solid\s+rgba\(125,\s*119,\s*105,\s*0\.12\)/);
  assert.match(focus, /box-shadow:\s*none\b/);
  assert.doesNotMatch(focus, /(?:blue|#00f(?:f{3})?|rgb\(\s*0\s*,\s*0\s*,\s*255\s*\))/i);
});
