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
    out[k] = { kind: v.expand ? 'macro' : 'kernel', desc: v.desc ?? '',
               ...(typeof v.expand === 'string' ? { expand: v.expand } : {}) };
  return out;
}

/* ---------------- parser ----------------
 * blocks split on blank lines; inline grammar:
 * text | #name | #name(args) | #name[body] | #name(args)[body], body nests. */

export function parse(src) {
  return String(src).split(/\n\s*\n/).map(b => b.trim()).filter(Boolean)
    .map(b => ({ name: 'para', args: [], children: parseInline(b.replace(/\n/g, ' ')) }));
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
        children = parseInline(inner); j = k;
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

export function render(tree, target, ctx = {}) {
  if (target === 'json') return JSON.stringify(toJsonTree(tree, ctx), null, 2);
  if (!TARGETS.includes(target)) throw new Error(`unknown target: ${target}`);
  ctx.links ??= [];
  const body = tree.map(p => target === 'html'
      ? renderHtmlPara(p.children, ctx)
      // strip leading blank lines but keep first-line indent (txt block terms)
      : renderNodes(p.children, target, ctx).replace(/^\n+/, '').trimEnd())
    .filter(Boolean)
    .join(target === 'html' ? '\n' : '\n\n');
  if (target === 'txt' && ctx.links.length)
    return body + '\n\n' + ctx.links.map((l, i) => `[${i + 1}] ${l}`).join('\n');
  return body;
}

/* html paragraphs: inline runs get <p class="prose">, block terms render bare */
function renderHtmlPara(children, ctx) {
  const out = []; let run = [];
  const flush = () => {
    if (!run.length) return;
    const s = renderNodes(run, 'html', ctx).trim();
    if (s) out.push(`<p class="prose">${s}</p>`);
    run = [];
  };
  for (const n of children) {
    if (n.name && !n.unknown && registry.get(n.name)?.block) {
      flush(); out.push(renderNodes([n], 'html', ctx).trim());
    } else run.push(n);
  }
  flush();
  return out.join('\n');
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
    : { term: n.name, ...(n.args.length ? { args: n.args } : {}),
        ...(n.unknown ? { unknown: true } : {}), children: clean(n.children) });
  return { type: 'fragment', ...(ctx.id ? { id: ctx.id } : {}),
           blocks: tree.map(p => ({ type: 'para', children: clean(p.children) })) };
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
  md:   (n, r) => `\n> ℹ ${r(n.children)}\n`,
  txt:  (n, r) => `\n  [${r(n.children)}]\n`,
});

defineTerm('quote', {
  desc: 'block quotation',
  block: true,
  html: (n, r) => `<blockquote>${r(n.children)}</blockquote>`,
  md:   (n, r) => `\n> ${r(n.children)}\n`,
  txt:  (n, r) => `\n  "${r(n.children)}"\n`,
});

defineTerm('code', {
  desc: 'inline code',
  html: (n, r) => `<code>${r(n.children)}</code>`,
  md:   (n, r) => '`' + r(n.children) + '`',
  txt:  (n, r) => `'${r(n.children)}'`,
});

defineTerm('figure', {
  desc: 'figure(src)[caption]',
  block: true,
  html: (n, r) => `<figure><div class="frag-ph">${escapeHtml(n.args[0] ?? 'attachment')}</div><figcaption>${r(n.children)}</figcaption></figure>`,
  md:   (n, r) => `\n![${r(n.children)}](${n.args[0] ?? ''})\n`,
  txt:  (n, r, ctx) => { ctx.links.push(n.args[0] ?? '?'); return `\n  {figure [${ctx.links.length}]: ${r(n.children)}}\n`; },
});

defineTerm('embed', {
  desc: 'embed(frag:id) — transclusion point, resolved at serve time',
  block: true,
  html: (n) => `<div class="frag-embed" data-ref="${escapeHtml(n.args[0] ?? '')}">⤷ ${escapeHtml(n.args[0] ?? '?')}</div>`,
  md:   (n) => `[→ ${n.args[0] ?? '?'}](${n.args[0] ?? ''})`,
  txt:  (n, _r, ctx) => { ctx.links.push(n.args[0] ?? '?'); return `(see [${ctx.links.length}])`; },
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
  md:   (n, r) => r(n.children),
  txt:  (n, r) => r(n.children),
});

defineTerm('item', passthrough('list/steps entry — meaningful inside #list or #steps'));
defineTerm('row',  passthrough('table row of #cell[…] — #row(header) marks a header row'));
defineTerm('cell', passthrough('table cell — meaningful inside #row'));

defineTerm('list', {
  desc: 'unordered list of #item[…] children',
  block: true,
  html: (n, r) => `<ul>${childTerms(n, 'item').map(it => `<li>${r(it.children)}</li>`).join('')}</ul>`,
  md:   (n, r) => '\n' + childTerms(n, 'item').map(it => `- ${r(it.children)}`).join('\n') + '\n',
  txt:  (n, r) => '\n' + childTerms(n, 'item').map(it => `  - ${r(it.children)}`).join('\n') + '\n',
});

defineTerm('steps', {
  desc: 'ordered list of #item[…] children',
  block: true,
  html: (n, r) => `<ol>${childTerms(n, 'item').map(it => `<li>${r(it.children)}</li>`).join('')}</ol>`,
  md:   (n, r) => '\n' + childTerms(n, 'item').map((it, i) => `${i + 1}. ${r(it.children)}`).join('\n') + '\n',
  txt:  (n, r) => '\n' + childTerms(n, 'item').map((it, i) => `  ${i + 1}. ${r(it.children)}`).join('\n') + '\n',
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
