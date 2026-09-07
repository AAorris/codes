# dockersite

A light Rust kernel that serves unauthenticated HTTP requests, routing each
request to a *function* — a forth-like micro language that assembles *fragments*
into HTML.

## Layout

```
kernel/          Rust source (std-only, no dependencies)
  main.rs        entry point; binds an address, optional CLI arg (default 0.0.0.0:8080)
  server.rs      std TCP listener, thread-per-connection, minimal HTTP/1.1 (GET/HEAD)
  router.rs      route via functions/route.f (method+path -> function name), 404/500 responses
  fnlang.rs      the function micro language interpreter
functions/       functions/route.f maps requests to functions; functions/404.f is the not-found page
dev/fragments/   content (.html), styles (.css), and assets (.svg), organized in folders
dev/fragments/meditation/   page folders, e.g. meditation/index.html (load "meditation")
dev/fragments/media/        shared assets, e.g. media/webring.svg ({{media/webring.svg}})
dev/templates/   page templates, e.g. template.html (with {{body}} and fragment includes)
Dockerfile.vercel  static musl build -> scratch runtime image
```

## How it works

Every request is routed by `functions/route.f` — a hot-reloaded *routing
function* that maps a method + path to a function name. The kernel re-reads it
on every request, so edits apply without restarting. It uses the `route` word,
which emits a function name when the method and path match (first match wins):

```
# functions/route.f
route "GET" "/" "default"    # GET /      -> functions/default.f
route "GET" "/i" "default"   # GET /i     -> functions/default.f
```

Patterns match exactly; a trailing `*` matches as a prefix (`"/blog*"`), and
`"*"` matches anything. Methods are case-insensitive, and `HEAD` is treated as
`GET`.

The resolved function is then interpreted with the request context. Functions
control the HTTP response directly — status, headers, and body:

```
# functions/default.f
status "200"          # set the response status
header "x-env" "development"   # set a response header
load "index"          # push dev/fragments/index.html (page sections) onto the stack
template "template"   # pop content, splice into {{body}} of dev/templates/template.html, push
write                 # pop the stack and append it to the response body
```

Fragment names may span folders: `load "meditation"` reads
`dev/fragments/meditation.html`, falling back to
`dev/fragments/meditation/index.html` when the page is a folder; template
includes may carry a folder too (`{{media/webring.svg}}` reads
`dev/fragments/media/webring.svg`). Names are restricted to `/`-separated
slugs (letters, digits, `_`, `-`) — no traversal.

The template is the page shell: `<head>` (with `{{style.css}}` inlined),
the site `<header>`, `{{body}}`, and the `<footer>` (with
`{{media/canada.svg}}`, `{{media/webring.svg}}`, `{{media/merveilles.svg}}`
inlined). After the `{{body}}` splice,
the template word also resolves fragment includes: any `{{name.css}}` or
`{{name.svg}}` token is replaced with the contents of
`dev/fragments/<name>.css|.svg`, folders included
(`{{media/webring.svg}}`). Styling lives in `dev/fragments/style.css` —
plain CSS tokens and patterns (no utility framework), derived from the
reference tailwind classes.

Words:

- `route "method" "pattern" "name"` — if the request matches, emit the function
  name; only meaningful in `functions/route.f`
- `status "code"` — set the response status code (100–599)
- `header "name" "value"` — add a response header
- `load "name"` — reads `dev/fragments/<name>.html` (or
  `dev/fragments/<name>/index.html` for folder pages) and pushes it onto the stack
- `template "name"` — pops the stack, reads `dev/templates/<name>.html`, replaces
  the first `{{body}}` with the popped content, resolves `{{name.css}}` /
  `{{name.svg}}` fragment includes, and pushes the assembled page
- `write` — pops the stack and appends it to the response body
- `ctx "method"` — pushes the request method onto the stack
- `ctx "path"` — pushes the request path onto the stack
- `resources "dir" "base"` — lists `dev/fragments/<dir>/*.html` (sorted by
  filename); pushes one `<li><a href="{base}{slug}">Title</a></li>` per file,
  where the slug is the filename and the title is the file's first `<h1>`/`<h2>`/
  `<h3>` text (falling back to the slug). Adding a resource is adding one file —
  no other file changes
- `resource "dir" "base"` — pops nothing; resolves the request path against
  `base` to a slug, reads `dev/fragments/<dir>/<slug>.html`, and pushes it. If
  the slug is invalid or the file is missing, sets the status to `404` and
  pushes `dev/fragments/404.html`
- `join` — pops everything on the stack and pushes it concatenated, bottom
  first

Anything else is an error, reported as a `500` with the function name and
message. `#` starts a comment. Strings are double-quoted with `\n`, `\t`,
`\\`, `\"` escapes. Stack fragments may be loaded and written more than once.
A response without an explicit `content-type` header defaults to
`text/html; charset=utf-8`.

## Development

```
cargo run
# or: cargo run -- 127.0.0.1:8080
```

The kernel re-reads `functions/` and `dev/fragments/` from disk on **every
request** — no restarts needed. Edit `functions/route.f`,
`functions/default.f`, or `dev/fragments/index.html`, refresh the browser, and
the change is live:

```
curl -s http://127.0.0.1:8080/
```

Routes are whatever `functions/route.f` says they are. When no route matches —
or `route.f` is missing or fails to run — the kernel falls back to
`functions/404.f`, whose own `status "404"` (or the default) sets the response
status (with a plain-text fallback if that file is missing too). Function
errors report the reason in the body as a `500`.

```
curl -si http://127.0.0.1:8080/
```

## Deploy

```
docker build -f Dockerfile.vercel -t dockersite .
```

The build compiles a static musl binary and copies it, along with `functions/`
and `dev/`, into a `scratch` runtime image. `production` serving (embedding or
caching) is a later object of focus; the container currently serves the same
reload-per-request behavior.

```
vercel --prod
```

Vercel can deploy natively with the Dockerfile.vercel