/* fragment.js — protected-hypertext fragment kernel.
 *
 * Zero dependencies, ESM. The unit of code is the *term*: each term
 * declares all of its target renderers adjacently, or declares itself
 * a macro that expands into other terms (and thus renders everywhere
 * for free).
 *
 * API:
 *   parse(src)                    -> tree (unexpanded)
 *   compile(src)                  -> { tree, unknown:Set<string> }   (parsed + macro-expanded)
 *   render(tree, target, ctx?)    -> string          target ∈ TARGETS
 *   renderAll(src, ctx?)          -> { html, md, txt, json, unknown }
 *   defineTerm(name, def)         -> void
 *   terms()                       -> { name: {kind, desc, expand?} } (introspection)
 *
 * Term definition shapes:
 *   { desc, expand: "aside[Note: {content}]" }          macro (template; {content}, {1}..{n})
 *   { desc, expand: (node) => "quote[...]" }            macro (function returning source)
 *   { desc, html(n,r,ctx), md(n,r,ctx), txt(n,r,ctx) }  kernel term (all targets required)
 *   block: true on a kernel term hoists its html output out of <p> wrapping
 *   verbatim: true captures the term's body raw (no term parsing, layout kept)
 *
 * ctx.width (default 72, 0 disables) sets the measure: txt/md prose is
 *   word-wrapped to it; #rule, #center, #right size against it.
 *
 * Renderer signature: (node, r, ctx) => string
 *   node : { name, args:[..], children:[..] }
 *   r    : render children in the current target: r(node.children)
 *   ctx  : per-render bag; ctx.links is provided for txt footnote trailers,
 *          callers may add e.g. ctx.caps, ctx.resolve for transclusion later.
 */

const TARGETS = ['html', 'md', 'txt', 'json'];
const MAX_DEPTH = 12;

/* ---------------- schema registry ---------------- */

const registry = new Map();

export function defineTerm(name, def) {
  if (!/^[a-zA-Z][a-zA-Z0-9_-]*$/.test(name)) throw new Error(`bad term name: ${name}`);
  if (def.expand == null) {
    for (const t of ['html', 'md', 'txt'])
      if (typeof def[t] !== 'function')
        throw new Error(`kernel term "${name}" must implement ${t} (all of html/md/txt), or provide expand`);
  }
  registry.set(name, def);
}

export function terms() {
  const out = {};
  for (const [k, v] of registry)
    out[k] = {
      kind: v.expand ? 'macro' : 'kernel', desc: v.desc ?? '',
      ...(typeof v.expand === 'string' ? { expand: v.expand } : {})
    };
  return out;
}

/* ---------------- colon sugar ----------------
 * #term: rest of line          → #term[rest of line]
 * #term:            (line end) → #term[ the deeper-indented lines below,
 *                                until a blank line or a dedent ]
 * Pure source-to-source rewrite into the bracket grammar, applied before
 * paragraph splitting — the parser and terms never see colons. Inline
 * bodies stop before an unmatched ']' so the sugar composes inside brackets.
 * Block bodies of verbatim terms (#pre, #codeblock) are captured raw into
 * `pockets` — through blank lines, common indent stripped — and re-attached
 * to the node as .raw after parsing, so layout survives newline collapse. */

const SUGAR = /#([a-zA-Z][a-zA-Z0-9_-]*)(?:\([^()]*\))?:/;
const indentOf = l => l.match(/^\s*/)[0].length;

function desugar(src, pockets = []) {
  const lines = String(src).split('\n'), out = [];
  for (let i = 0; i < lines.length; i++) {
    let line = lines[i], m;
    while ((m = SUGAR.exec(line))) {
      const head = m[0].slice(0, -1), pre = line.slice(0, m.index);
      const rest = line.slice(m.index + m[0].length);
      if (rest.trim()) {                 // inline: body runs to end of line
        let d = 0, k = 0;
        while (k < rest.length && !(rest[k] === ']' && d === 0)) {
          if (rest[k] === '[') d++; else if (rest[k] === ']') d--; k++;
        }
        line = pre + head + '[' + rest.slice(0, k).trim() + ']' + rest.slice(k);
      } else if (registry.get(m[1])?.verbatim) {  // verbatim block: pocket the raw lines
        const indent = indentOf(line), body = [];
        while (i + 1 < lines.length
          && (!lines[i + 1].trim() || indentOf(lines[i + 1]) > indent))
          body.push(lines[++i]);
        while (body.length && !body.at(-1).trim()) { body.pop(); i--; }
        const strip = Math.min(...body.filter(l => l.trim()).map(indentOf), Infinity);
        pockets.push(body.map(l => l.slice(strip)).join('\n'));
        const ref = `%%${pockets.length - 1}%%`;
        line = pre + (head.endsWith(')') ? head.slice(0, -1) + ',' + ref + ')' : head + '(' + ref + ')');
      } else {                           // block: body is the deeper-indented lines below
        const indent = indentOf(line), body = [];
        while (i + 1 < lines.length && lines[i + 1].trim()
          && indentOf(lines[i + 1]) > indent)
          body.push(lines[++i]);
        line = pre + head + '[' + desugar(body.join('\n'), pockets) + ']';
      }
    }
    out.push(line);
  }
  return out.join('\n');
}

/* ---------------- parser ----------------
 * blocks split on blank lines; inline grammar:
 * text | #name | #name(args) | #name[body] | #name(args)[body], body nests.
 * colon sugar (above) desugars to this grammar first. */

export function parse(src) {
  const pockets = [];
  const tree = desugar(src, pockets).split(/\n\s*\n/).map(b => b.trim()).filter(Boolean)
    .map(b => ({ name: 'para', args: [], children: parseInline(b.replace(/\n/g, ' ')) }));
  if (pockets.length) tree.forEach(p => attachRaw(p.children, pockets));
  return tree;
}

function attachRaw(nodes, pockets) {
  for (const n of nodes) {
    if (n.text != null) continue;
    const j = n.args.findIndex(a => /^%%\d+%%$/.test(a));
    if (j >= 0) { n.raw = pockets[+n.args[j].slice(2, -2)]; n.args.splice(j, 1); }
    attachRaw(n.children, pockets);
  }
}

function parseInline(s) {
  const nodes = []; let i = 0, buf = '';
  const flush = () => { if (buf) { nodes.push({ text: buf }); buf = ''; } };
  while (i < s.length) {
    if (s[i] === '#' && /[a-zA-Z]/.test(s[i + 1] || '')) {
      let j = i + 1, name = '';
      while (j < s.length && /[a-zA-Z0-9_-]/.test(s[j])) name += s[j++];
      let args = [];
      if (s[j] === '(') {
        const [inner, k] = balanced(s, j, '(', ')');
        args = inner.split(',').map(x => x.trim()).filter(Boolean); j = k;
      }
      let children = [];
      if (s[j] === '[') {
        const [inner, k] = balanced(s, j, '[', ']');
        children = registry.get(name)?.verbatim ? [{ text: inner }] : parseInline(inner); j = k;
      }
      flush(); nodes.push({ name, args, children }); i = j;
    } else buf += s[i++];
  }
  flush(); return nodes;
}

function balanced(s, open, oc, cc) {
  let d = 1, out = '', k = open + 1;
  while (k < s.length && d > 0) {
    if (s[k] === oc) d++; else if (s[k] === cc) d--;
    if (d > 0) out += s[k]; k++;
  }
  return [out, k];
}

/* ---------------- expansion ---------------- */

export function compile(src) {
  const unknown = new Set();
  const tree = parse(src).map(p => ({ ...p, children: expandNodes(p.children, 0, unknown) }));
  return { tree, unknown };
}

function expandNodes(nodes, depth, unknown) {
  if (depth > MAX_DEPTH) return [{ text: '[expansion depth exceeded]' }];
  const out = [];
  for (const n of nodes) {
    if (n.text != null) { out.push(n); continue; }
    const def = registry.get(n.name);
    if (!def) { unknown.add(n.name); out.push({ ...n, unknown: true, children: expandNodes(n.children, depth + 1, unknown) }); continue; }
    if (!def.expand) { out.push({ ...n, children: expandNodes(n.children, depth + 1, unknown) }); continue; }
    const srcStr = typeof def.expand === 'function'
      ? def.expand(n)
      : substitute(def.expand, n);
    out.push(...expandNodes(parseInline('#' + srcStr.replace(/^#/, '')), depth + 1, unknown));
  }
  return out;
}

function substitute(tpl, node) {
  let s = tpl.replaceAll('{content}', reserialize(node.children));
  node.args.forEach((a, i) => { s = s.replaceAll(`{${i + 1}}`, a); });
  return s;
}

function reserialize(children) {
  return children.map(c => c.text != null ? c.text
    : '#' + c.name + (c.args.length ? `(${c.args.join(',')})` : '') + `[${reserialize(c.children)}]`).join('');
}

/* ---------------- rendering ---------------- */

export const escapeHtml = s =>
  s.replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

/* greedy word-wrap to width; every line prefixed with indent (counted
 * against width). width 0 disables wrapping. */
export function wrap(s, width, indent = '') {
  s = s.replace(/\s+/g, ' ').trim();
  if (!width) return indent + s;
  const out = []; let line = '';
  for (const w of s.split(' ')) {
    if (line && (indent + line + ' ' + w).length > width) { out.push(indent + line); line = w; }
    else line = line ? line + ' ' + w : w;
  }
  if (line) out.push(indent + line);
  return out.join('\n');
}

export function render(tree, target, ctx = {}) {
  if (target === 'json') return JSON.stringify(toJsonTree(tree, ctx), null, 2);
  if (!TARGETS.includes(target)) throw new Error(`unknown target: ${target}`);
  ctx.links ??= [];
  ctx.width ??= 72;                 // measure for txt/md wrapping; 0 disables
  let body = tree.map(p => renderPara(p.children, target, ctx))
    .filter(Boolean)
    .join(target === 'html' ? '\n' : '\n\n');
  if (target === 'txt') {
    body = body.split('\n').map(l => fillLine(l, ctx.width || 72)).join('\n');
    if (ctx.links.length)
      body += '\n\n' + ctx.links.map((l, i) => `[${i + 1}] ${l}`).join('\n');
  }
  return body;
}

/* #fill glue: txt renderers emit a sentinel; once lines are final, each
 * line's leftover width is distributed across its sentinels. */
const FILL = '\x00';
function fillLine(l, width) {
  const parts = l.split(FILL);
  if (parts.length === 1) return l;
  const gaps = parts.length - 1, free = Math.max(gaps, width - parts.join('').length);
  const base = Math.floor(free / gaps), rem = free % gaps;
  return parts.reduce((acc, p, i) => acc + ' '.repeat(base + (i <= rem ? 1 : 0)) + p);
}

/* paragraph assembly: block terms render bare between inline runs.
 * html wraps runs in <p class="prose">; txt/md word-wrap them at ctx.width. */
function renderPara(children, target, ctx) {
  const segs = []; let run = [];
  const flush = () => {
    if (!run.length) return;
    const s = renderNodes(run, target, ctx).replace(/\s+/g, ' ').trim();
    if (s) segs.push(target === 'html' ? `<p class="prose">${s}</p>` : wrap(s, ctx.width));
    run = [];
  };
  for (const n of children) {
    if (n.name && !n.unknown && registry.get(n.name)?.block) {
      flush();
      segs.push(renderNodes([n], target, ctx).replace(/^\n+/, '').trimEnd());
    } else run.push(n);
  }
  flush();
  return segs.join(target === 'html' ? '\n' : '\n\n');
}

function renderNodes(nodes, target, ctx) {
  const r = ns => renderNodes(ns, target, ctx);
  return nodes.map(n => {
    if (n.text != null) return target === 'html' ? escapeHtml(n.text) : n.text;
    if (n.unknown) return target === 'html'
      ? `<span class="frag-err">#${escapeHtml(n.name)}[?]</span>`
      : target === 'md' ? `#${n.name}[?]` : '';
    const def = registry.get(n.name);
    return def[target](n, r, ctx);
  }).join('');
}

function toJsonTree(tree, ctx) {
  const clean = ns => ns.map(n => n.text != null ? { text: n.text }
    : {
      term: n.name, ...(n.args.length ? { args: n.args } : {}),
      ...(n.raw != null ? { raw: n.raw } : {}),
      ...(n.unknown ? { unknown: true } : {}), children: clean(n.children)
    });
  return {
    type: 'fragment', ...(ctx.id ? { id: ctx.id } : {}),
    blocks: tree.map(p => ({ type: 'para', children: clean(p.children) }))
  };
}

export function renderAll(src, ctx = {}) {
  const { tree, unknown } = compile(src);
  const out = { unknown };
  for (const t of TARGETS) out[t] = render(tree, t, { ...ctx, links: [] });
  return out;
}

export { TARGETS };

/* ---------------- built-in terms ----------------
 * Each term's renderers live together: html, md, txt side by side.
 * This adjacency is the point — an #embed's whole behavior is one object. */

defineTerm('aside', {
  desc: 'marginal note',
  block: true,
  html: (n, r) => `<aside>${r(n.children)}</aside>`,
  md: (n, r, ctx) => {
    const s = `ℹ ${r(n.children)}`;
    return '\n' + (s.includes('\n') ? '> ' + s : wrap(s, ctx.width, '> ')) + '\n';
  },
  txt: (n, r, ctx) => {
    const s = `[${r(n.children)}]`;
    return '\n' + (s.includes('\n') ? '  ' + s : wrap(s, ctx.width, '  ')) + '\n';
  },
});

defineTerm('quote', {
  desc: 'block quotation',
  block: true,
  html: (n, r) => `<blockquote>${r(n.children)}</blockquote>`,
  md: (n, r, ctx) => {
    const s = r(n.children);
    return '\n' + (s.includes('\n') ? '> ' + s : wrap(s, ctx.width, '> ')) + '\n';
  },
  txt: (n, r, ctx) => {
    const s = `"${r(n.children)}"`;
    return '\n' + (s.includes('\n') ? '  ' + s : wrap(s, ctx.width, '  ')) + '\n';
  },
});

defineTerm('code', {
  desc: 'inline code (verbatim body)',
  verbatim: true,
  html: (n, r) => `<code>${r(n.children)}</code>`,
  md: (n, r) => '`' + r(n.children) + '`',
  txt: (n, r) => `'${r(n.children)}'`,
});

defineTerm('figure', {
  desc: 'figure(src)[caption]',
  block: true,
  html: (n, r) => `<figure><div class="frag-ph">${escapeHtml(n.args[0] ?? 'attachment')}</div><figcaption>${r(n.children)}</figcaption></figure>`,
  md: (n, r) => `\n![${r(n.children)}](${n.args[0] ?? ''})\n`,
  txt: (n, r, ctx) => { ctx.links.push(n.args[0] ?? '?'); return `\n  {figure [${ctx.links.length}]: ${r(n.children)}}\n`; },
});

defineTerm('embed', {
  desc: 'embed(frag:id) — transclusion point, resolved at serve time',
  block: true,
  html: (n) => `<div class="frag-embed" data-ref="${escapeHtml(n.args[0] ?? '')}">⤷ ${escapeHtml(n.args[0] ?? '?')}</div>`,
  md: (n) => `[→ ${n.args[0] ?? '?'}](${n.args[0] ?? ''})`,
  txt: (n, _r, ctx) => { ctx.links.push(n.args[0] ?? '?'); return `(see [${ctx.links.length}])`; },
});

defineTerm('callout', {
  desc: 'macro → aside',
  expand: 'aside[Note: {content}]',
});

/* structural containers: list/steps/table render their item/row children
 * directly (parent-driven), so ordered numbering and column widths are
 * computable. item/row/cell are registered so they never read as unknown;
 * standalone they just pass their children through. */

const childTerms = (node, ...names) =>
  node.children.filter(c => c.name && names.includes(c.name));

const passthrough = desc => ({
  desc,
  html: (n, r) => r(n.children),
  md: (n, r) => r(n.children),
  txt: (n, r) => r(n.children),
});

defineTerm('item', passthrough('list/steps entry — meaningful inside #list or #steps'));
defineTerm('row', passthrough('table row of #cell[…] — #row(header) marks a header row'));
defineTerm('cell', passthrough('table cell — meaningful inside #row'));

/* txt list item with hanging indent: first line carries the marker,
 * continuations align under the text; multiline content passes through */
function hangItem(marker, s, width) {
  if (s.includes('\n')) return marker + s;
  const pad = ' '.repeat(marker.length);
  return wrap(s, (width || 72) - marker.length).split('\n')
    .map((l, i) => (i ? pad : marker) + l).join('\n');
}

defineTerm('list', {
  desc: 'unordered list of #item[…] children',
  block: true,
  html: (n, r) => `<ul>${childTerms(n, 'item').map(it => `<li>${r(it.children)}</li>`).join('')}</ul>`,
  md: (n, r) => '\n' + childTerms(n, 'item').map(it => `- ${r(it.children)}`).join('\n') + '\n',
  txt: (n, r, ctx) => '\n' + childTerms(n, 'item')
    .map(it => hangItem('  - ', r(it.children), ctx.width)).join('\n') + '\n',
});

defineTerm('steps', {
  desc: 'ordered list of #item[…] children',
  block: true,
  html: (n, r) => `<ol>${childTerms(n, 'item').map(it => `<li>${r(it.children)}</li>`).join('')}</ol>`,
  md: (n, r) => '\n' + childTerms(n, 'item').map((it, i) => `${i + 1}. ${r(it.children)}`).join('\n') + '\n',
  txt: (n, r) => '\n' + childTerms(n, 'item').map((it, i) => `  ${i + 1}. ${r(it.children)}`).join('\n') + '\n',
});

/* split children once on the first ';' in top-level text: [left, right] */
function splitPair(children) {
  for (let i = 0; i < children.length; i++) {
    const c = children[i];
    const k = c.text != null ? c.text.indexOf(';') : -1;
    if (k < 0) continue;
    return [[...children.slice(0, i), { text: c.text.slice(0, k) }],
    [{ text: c.text.slice(k + 1) }, ...children.slice(i + 1)]];
  }
  return [children, []];
}

defineTerm('fill', {
  desc: 'elastic space: #fill[content] pushes content to the right margin',
  html: (n, r) => `<span class="frag-fill">${r(n.children)}</span>`,
  md: (n, r) => ` — ${r(n.children)}`,
  txt: (n, r) => FILL + r(n.children),
});

defineTerm('ledger', {
  desc: 'macro → #list of #item[left #fill[right]]; items split on first ";"',
  expand: n => 'list[' + childTerms(n, 'item').map(it => {
    const [l, rt] = splitPair(it.children);
    const left = reserialize(l).trim(), right = reserialize(rt).trim();
    return `#item[${left}${right ? ` #fill[${right}]` : ''}]`;
  }).join('') + ']',
});

/* rows as { header, cells[] }, ragged rows padded to the widest */
function tableRows(n, r) {
  const rows = childTerms(n, 'row').map(row => ({
    header: row.args[0] === 'header',
    cells: childTerms(row, 'cell').map(c => r(c.children)),
  }));
  const width = Math.max(1, ...rows.map(rw => rw.cells.length));
  for (const rw of rows) while (rw.cells.length < width) rw.cells.push('');
  return rows;
}

defineTerm('table', {
  desc: 'table of #row[#cell[…]…] children; #row(header) for a header row',
  block: true,
  html: (n, r) => {
    const rows = tableRows(n, r);
    const tr = (rw, tag) => `<tr>${rw.cells.map(c => `<${tag}>${c}</${tag}>`).join('')}</tr>`;
    const head = rows.filter(rw => rw.header), body = rows.filter(rw => !rw.header);
    return '<table>'
      + (head.length ? `<thead>${head.map(rw => tr(rw, 'th')).join('')}</thead>` : '')
      + `<tbody>${body.map(rw => tr(rw, 'td')).join('')}</tbody></table>`;
  },
  md: (n, r) => {
    const rows = tableRows(n, r);
    if (!rows.length) return '';
    const cols = rows[0].cells.length;
    const line = cells => `| ${cells.map(c => c.replaceAll('|', '\\|')).join(' | ')} |`;
    const head = rows.find(rw => rw.header);
    const body = rows.filter(rw => rw !== head);
    return '\n' + line(head ? head.cells : Array(cols).fill('')) + '\n'
      + `|${' --- |'.repeat(cols)}\n`
      + body.map(rw => line(rw.cells)).join('\n') + '\n';
  },
  txt: (n, r) => {
    const rows = tableRows(n, r);
    if (!rows.length) return '';
    const widths = rows[0].cells.map((_, i) => Math.max(...rows.map(rw => rw.cells[i].length)));
    const line = cells => ('  ' + cells.map((c, i) => c.padEnd(widths[i])).join('  ')).trimEnd();
    const out = [];
    for (const rw of rows) {
      out.push(line(rw.cells));
      if (rw.header) out.push(line(widths.map(w => '-'.repeat(w))));
    }
    return '\n' + out.join('\n') + '\n';
  },
});

/* ---------------- typesetting terms ---------------- */

defineTerm('rule', {
  desc: 'thematic break',
  block: true,
  html: () => '<hr>',
  md: () => '\n---\n',
  txt: (n, _r, ctx) => '\n' + '-'.repeat(ctx.width || 72) + '\n',
});

for (const [name, lvl, u] of [['title', 1, '='], ['section', 2, '-'], ['sub', 3, '~']])
  defineTerm(name, {
    desc: `heading (h${lvl})`,
    block: true,
    html: (n, r) => `<h${lvl}>${r(n.children)}</h${lvl}>`,
    md: (n, r) => `\n${'#'.repeat(lvl)} ${r(n.children)}\n`,
    txt: (n, r) => { const s = r(n.children); return `\n${s}\n${u.repeat(s.length)}\n`; },
  });

/* alignment: html via class, txt pads within ctx.width, md degrades to prose */
const aligned = (name, pad) => defineTerm(name, {
  desc: `${name}-aligned block (md degrades to plain paragraph)`,
  block: true,
  html: (n, r) => `<p class="prose frag-${name}">${r(n.children)}</p>`,
  md: (n, r) => `\n${r(n.children)}\n`,
  txt: (n, r, ctx) => {
    const w = ctx.width || 72;
    return '\n' + wrap(r(n.children), w).split('\n')
      .map(l => ' '.repeat(Math.max(0, pad(w, l.length))) + l).join('\n') + '\n';
  },
});
aligned('center', (w, len) => (w - len) >> 1);
aligned('right', (w, len) => w - len);

/* verbatim: block bodies (colon form) arrive raw via n.raw; bracket
 * bodies stay a single unparsed text child. */
const rawOf = n => n.raw ?? n.children.map(c => c.text ?? '').join('');
const indented = n => '\n' + rawOf(n).split('\n').map(l => ('  ' + l).trimEnd()).join('\n') + '\n';

defineTerm('pre', {
  desc: 'preformatted text — #pre: keeps the indented lines below verbatim',
  block: true, verbatim: true,
  html: n => `<pre>${escapeHtml(rawOf(n))}</pre>`,
  md: n => '\n```\n' + rawOf(n) + '\n```\n',
  txt: indented,
});

defineTerm('codeblock', {
  desc: 'codeblock(lang) — fenced code block, verbatim like #pre',
  block: true, verbatim: true,
  html: n => `<pre><code${n.args[0] ? ` class="language-${escapeHtml(n.args[0])}"` : ''}>${escapeHtml(rawOf(n))}</code></pre>`,
  md: n => '\n```' + (n.args[0] ?? '') + '\n' + rawOf(n) + '\n```\n',
  txt: indented,
});
