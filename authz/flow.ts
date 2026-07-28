import crypto from "node:crypto";
import { base64url, decodeJwt, fromBase64url } from "./jwt.ts";
import {
  createAuthorizationServer,
  createResourceServer,
  type AuthorizeRequest,
  type Client,
  type TokenRequest,
} from "./model.ts";

function log(hop: string, data: unknown) {
  console.log(`\n${hop}`);
  console.log(typeof data === "string" ? `  ${data}` : data);
}

function expectFailure(name: string, fn: () => unknown) {
  try {
    fn();
  } catch (err) {
    console.log(`[rejected] ${name} — ${(err as Error).message}`);
    return;
  }
  throw new Error(`negative case "${name}" unexpectedly succeeded`);
}

// ---- setup: one public client, one AS, one RS ------------------------------

const client: Client = {
  client_id: "demo-client",
  redirect_uris: ["https://client.example/callback"],
  authenticate: () => true, // public client: no credentials (client-auth seam)
};

const as = createAuthorizationServer([client]);
const rs = createResourceServer(as.publicKey); // the only thing the RS gets from the AS

// ---- happy path ------------------------------------------------------------

// Front channel: the client sends the resource owner's browser to the AS.
const state = crypto.randomUUID();
const authorizeUrl = `https://auth.example/authorize?${new URLSearchParams({
  response_type: "code",
  client_id: client.client_id,
  redirect_uri: client.redirect_uris[0],
  state,
})}`;
log("client -> browser -> AS  (front channel)", authorizeUrl);

// The AS receives those query params, authenticates the user out of band,
// and gets a consent decision from the resource owner.
const authorizeRequest = Object.fromEntries(
  new URL(authorizeUrl).searchParams,
) as AuthorizeRequest;
const result = as.authorize(authorizeRequest, { approved: true, sub: "aaron" });
log(
  "AS -> browser -> client  (front channel redirect)",
  `${result.redirect_uri}?${new URLSearchParams({ code: result.code, state: result.state })}`,
);

// The client trusts nothing off the front channel until state matches.
if (result.state !== state) throw new Error("client: state mismatch");

// Back channel: client -> AS directly, browser never sees this.
const tokenRequest: TokenRequest = {
  grant_type: "authorization_code",
  code: result.code,
  redirect_uri: client.redirect_uris[0],
  client_id: client.client_id,
};
const tokenResponse = as.token(tokenRequest);
log("AS -> client  (back channel)", tokenResponse);
log("client can decode the token but holds no key to verify or mint", decodeJwt(tokenResponse.access_token));

// The client presents the bearer token to the RS.
const resource = rs.handleRequest(`Bearer ${tokenResponse.access_token}`);
log("RS -> client", resource);

// ---- negative cases --------------------------------------------------------

console.log("\nnegative cases:");

expectFailure("reused code", () => as.token(tokenRequest));

expectFailure("expired code", () => {
  const expiredAs = createAuthorizationServer([client], { codeTtl: -10 });
  const r = expiredAs.authorize(authorizeRequest, { approved: true, sub: "aaron" });
  expiredAs.token({ ...tokenRequest, code: r.code });
});

expectFailure("redirect_uri mismatch at /token", () => {
  const r = as.authorize(authorizeRequest, { approved: true, sub: "aaron" });
  as.token({ ...tokenRequest, code: r.code, redirect_uri: "https://evil.example/callback" });
});

expectFailure("tampered access token", () => {
  const [header, payload, sig] = tokenResponse.access_token.split(".");
  const claims = JSON.parse(fromBase64url(payload).toString());
  claims.sub = "mallory";
  const forged = `${header}.${base64url(Buffer.from(JSON.stringify(claims)))}.${sig}`;
  rs.handleRequest(`Bearer ${forged}`);
});

expectFailure("authorization code presented to the RS", () => {
  const r = as.authorize(authorizeRequest, { approved: true, sub: "aaron" });
  rs.handleRequest(`Bearer ${r.code}`);
});

expectFailure("state mismatch", () => {
  const forged = { ...result, state: "attacker-chosen" };
  if (forged.state !== state) throw new Error("client: state mismatch");
});

expectFailure("resource owner denies", () => {
  as.authorize(authorizeRequest, { approved: false, sub: "aaron" });
});
