/* Fragment: read bracket terms, emit four proofs. */

const TERMS = new Set(`aside quote code figure lit embed item row cell list steps
fill table rule title section sub center right pre codeblock`.split(/\s+/));
const RAW = new Set(["code", "lit", "pre", "codeblock"]);
const BLOCK = new Set(["aside", "quote", "figure", "lit", "embed", "list", "steps",
	"table", "rule", "title", "section", "sub", "center", "right", "pre", "codeblock"]);
const FILL = "\0";

/* string | [name, args, children] */
function read(source, unknown) {
	return String(source).split(/\n\s*\n/).map((p) => scan(p.trim().replace(/\n/g, " "), unknown));
}

function scan(source, unknown) {
	const out = [];
	let text = "";
	let i = 0;
	const flush = () => { if (text) { out.push(text); text = ""; } };
	while (i < source.length) {
		if (source[i] !== "#" || !/[a-zA-Z]/.test(source[i + 1] || "")) {
			text += source[i++]; continue;
		}
		let j = i + 1;
		let name = "";
		while (/[a-zA-Z0-9_-]/.test(source[j] || "")) name += source[j++];
		let args = [];
		let kids = [];
		if (source[j] === "(") {
			const b = take(source, j, "(", ")");
			args = b[0].split(",").map((x) => x.trim()).filter(Boolean);
			j = b[1];
		}
		if (source[j] === "[") {
			const b = take(source, j, "[", "]");
			kids = RAW.has(name) ? [b[0]] : scan(b[0], unknown);
			j = b[1];
		}
		flush();
		if (!TERMS.has(name)) unknown.add(name);
		out.push([name, args, kids]);
		i = j;
	}
	flush();
	return out;
}

function take(source, i, open, close) {
	let depth = 1;
	let body = "";
	let cursor = i + 1;
	for (; cursor < source.length && depth; cursor++) {
		if (source[cursor] === open) depth++;
		else if (source[cursor] === close) depth--;
		if (depth) body += source[cursor];
	}
	return [body, cursor];
}

const esc = (s) => s.replace(/[&<>"]/g, (c) =>
	({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const kids = (n, name) => n[2].filter((x) => typeof x !== "string" && x[0] === name);
const raw = (n) => n[2].map((x) => typeof x === "string" ? x : "").join("");

function wrap(source, width = 72, indent = "") {
	const words = source.replace(/\s+/g, " ").trim().split(" ");
	if (!width) return indent + words.join(" ");
	const out = [];
	let line = "";
	for (const word of words) {
		if (line && `${indent}${line} ${word}`.length > width) {
			out.push(indent + line); line = word;
		} else line += (line ? " " : "") + word;
	}
	if (line) out.push(indent + line);
	return out.join("\n");
}

function document(tree, target, ctx) {
	let out = tree.map((p) => paragraph(p, target, ctx)).filter(Boolean)
		.join(target === "html" ? "\n" : "\n\n");
	if (target !== "txt") return out;
	out = out.split("\n").map((line) => fill(line, ctx.width)).join("\n");
	if (ctx.links.length) out += `\n\n${ctx.links.map((x, i) => `[${i + 1}] ${x}`).join("\n")}`;
	return out;
}

function paragraph(nodes, target, ctx) {
	const out = [];
	let run = [];
	const flush = () => {
		if (!run.length) return;
		const s = emitAll(run, target, ctx).replace(/\s+/g, " ").trim();
		if (s) out.push(target === "html" ? `<p class="prose">${s}</p>` : wrap(s, ctx.width));
		run = [];
	};
	for (const n of nodes) {
		if (typeof n !== "string" && TERMS.has(n[0]) && BLOCK.has(n[0])) {
			flush(); out.push(emit(n, target, ctx).replace(/^\n+/, "").trimEnd());
		} else run.push(n);
	}
	flush();
	return out.join(target === "html" ? "\n" : "\n\n");
}

const emitAll = (nodes, target, ctx) => nodes.map((n) =>
	typeof n === "string" ? (target === "html" ? esc(n) : n) : emit(n, target, ctx)).join("");

function emit(n, target, ctx) {
	const name = n[0];
	const args = n[1];
	const body = n[2];
	const r = (x = body) => emitAll(x, target, ctx);
	if (!TERMS.has(name)) return target === "html"
		? `<span class="frag-err">#${esc(name)}[?]</span>` : target === "md" ? `#${name}[?]` : "";

	switch (name) {
		case "aside": {
			if (target === "html") return `<aside>${r()}</aside>`;
			const s = target === "md" ? `ℹ ${r()}` : `[${r()}]`;
			return `\n${wrap(s, ctx.width, target === "md" ? "> " : "  ")}\n`;
		}
		case "quote": {
			if (target === "html") return `<blockquote>${r()}</blockquote>`;
			return `\n${wrap(target === "txt" ? `"${r()}"` : r(), ctx.width,
				target === "md" ? "> " : "  ")}\n`;
		}
		case "code":
			return target === "html" ? `<code>${r()}</code>` : target === "md" ? `\`${r()}\`` : `'${r()}'`;
		case "figure":
			if (target === "html") return `<figure><div class="frag-ph">${esc(args[0] || "attachment")}</div><figcaption>${r()}</figcaption></figure>`;
			if (target === "md") return `\n![${r()}](${args[0] || ""})\n`;
			return link(ctx, args[0], (i) => `\n  {figure [${i}]: ${r()}}\n`);
		case "embed":
			if (target === "html") return `<div class="frag-embed" data-ref="${esc(args[0] || "")}">⤷ ${esc(args[0] || "?")}</div>`;
			if (target === "md") return `[→ ${args[0] || "?"}](${args[0] || ""})`;
			return link(ctx, args[0], (i) => `(see [${i}])`);
		case "lit": {
			const source = raw(n).trim();
			const proof = renderAll(source, ctx)[target];
			if (target === "html") return `<figure class="frag-lit"><pre class="frag-lit-src">${esc(source)}</pre><div class="frag-lit-out">${proof}</div></figure>`;
			if (target === "md") return `\n\`\`\`frag\n${source}\n\`\`\`\n\n${proof}\n`;
			const line = "-".repeat(Math.min(40, ctx.width));
			return `\n${line}\n${source}\n${line}\n${proof}\n`;
		}
		case "item": case "row": case "cell": return r();
		case "list": return list(n, target, ctx, false);
		case "steps": return list(n, target, ctx, true);
		case "fill":
			return target === "html" ? `<span class="frag-fill">${r()}</span>`
				: target === "md" ? ` — ${r()}` : FILL + r();
		case "table": return table(n, target, ctx);
		case "rule":
			return target === "html" ? "<hr>" : target === "md" ? "\n---\n" : `\n${"-".repeat(ctx.width)}\n`;
		case "title": case "section": case "sub": {
			const level = ["title", "section", "sub"].indexOf(name) + 1;
			const s = r();
			if (target === "html") return `<h${level}>${s}</h${level}>`;
			if (target === "md") return `\n${"#".repeat(level)} ${s}\n`;
			return `\n${s}\n${["=", "-", "~"][level - 1].repeat(s.length)}\n`;
		}
		case "center": case "right": {
			if (target === "html") return `<p class="prose frag-${name}">${r()}</p>`;
			if (target === "md") return `\n${r()}\n`;
			const lines = wrap(r(), ctx.width).split("\n").map((s) => {
				const pad = name === "center" ? (ctx.width - s.length) >> 1 : ctx.width - s.length;
				return `${" ".repeat(Math.max(0, pad))}${s}`;
			}).join("\n");
			return `\n${lines}\n`;
		}
		case "pre": case "codeblock": {
			const source = raw(n);
			if (target === "txt") return `\n${source.split("\n").map((s) => `  ${s}`.trimEnd()).join("\n")}\n`;
			if (target === "md") return `\n\`\`\`${name === "codeblock" ? args[0] || "" : ""}\n${source}\n\`\`\`\n`;
			const lang = name === "codeblock" && args[0] ? ` class="language-${esc(args[0])}"` : "";
			return name === "pre" ? `<pre>${esc(source)}</pre>` : `<pre><code${lang}>${esc(source)}</code></pre>`;
		}
	}
}

function link(ctx, href, make) {
	ctx.links.push(href || "?");
	return make(ctx.links.length);
}

function list(n, target, ctx, ordered) {
	const items = kids(n, "item");
	const r = (x) => emitAll(x[2], target, ctx);
	if (target === "html") return `<${ordered ? "ol" : "ul"}>${items.map((x) =>
		`<li>${r(x)}</li>`).join("")}</${ordered ? "ol" : "ul"}>`;
	if (target === "md") return `\n${items.map((x, i) =>
		`${ordered ? `${i + 1}.` : "-"} ${r(x)}`).join("\n")}\n`;
	return `\n${items.map((x, i) => hang(ordered ? `  ${i + 1}. ` : "  - ", r(x), ctx.width))
		.join("\n")}\n`;
}

function hang(mark, source, width) {
	const lines = wrap(source, width - mark.length).split("\n");
	return lines.map((s, i) => `${i ? " ".repeat(mark.length) : mark}${s}`).join("\n");
}

function rows(n, target, ctx) {
	const out = kids(n, "row").map((row) => ({
		head: row[1][0] === "header",
		cells: kids(row, "cell").map((cell) => emitAll(cell[2], target, ctx)),
	}));
	const width = Math.max(1, ...out.map((row) => row.cells.length));
	for (const row of out) while (row.cells.length < width) row.cells.push("");
	return out;
}

function table(n, target, ctx) {
	const rs = rows(n, target, ctx);
	if (target === "html") {
		const tr = (row, tag) => `<tr>${row.cells.map((x) => `<${tag}>${x}</${tag}>`).join("")}</tr>`;
		const head = rs.filter((x) => x.head);
		const body = rs.filter((x) => !x.head);
		return `<table>${head.length ? `<thead>${head.map((x) => tr(x, "th")).join("")}</thead>` : ""}<tbody>${body.map((x) => tr(x, "td")).join("")}</tbody></table>`;
	}
	if (!rs.length) return "";
	if (target === "md") {
		const line = (cells) => `| ${cells.map((x) => x.replaceAll("|", "\\|")).join(" | ")} |`;
		const head = rs.find((x) => x.head) || { cells: Array(rs[0].cells.length).fill("") };
		return `\n${line(head.cells)}\n|${" --- |".repeat(head.cells.length)}\n${rs.filter((x) => x !== head).map((x) => line(x.cells)).join("\n")}\n`;
	}
	const widths = rs[0].cells.map((_, i) => Math.max(...rs.map((x) => x.cells[i].length)));
	const line = (cells) => `  ${cells.map((x, i) => x.padEnd(widths[i])).join("  ")}`.trimEnd();
	const out = [];
	for (const row of rs) { out.push(line(row.cells)); if (row.head) out.push(line(widths.map((x) => "-".repeat(x)))); }
	return `\n${out.join("\n")}\n`;
}

function fill(line, width) {
	const parts = line.split(FILL);
	if (parts.length === 1) return line;
	const free = Math.max(parts.length - 1, width - parts.join("").length);
	const gap = Math.floor(free / (parts.length - 1));
	let rem = free % (parts.length - 1);
	return parts.reduce((out, part) => out + " ".repeat(gap + (rem-- > 0 ? 1 : 0)) + part);
}

function json(tree) {
	const clean = (nodes) => nodes.map((n) => typeof n === "string" ? { text: n } : ({
		term: n[0], ...(n[1].length ? { args: n[1] } : {}),
		...(!TERMS.has(n[0]) ? { unknown: true } : {}), children: clean(n[2]),
	}));
	return { type: "fragment", blocks: tree.map((children) => ({ type: "para", children: clean(children) })) };
}

export function renderAll(source, options = {}) {
	const unknown = new Set();
	const tree = read(source, unknown);
	const make = () => ({ width: options.width ?? 72, links: [] });
	return {
		html: document(tree, "html", make()),
		md: document(tree, "md", make()),
		txt: document(tree, "txt", make()),
		json: JSON.stringify(json(tree), null, 2),
		unknown,
	};
}
