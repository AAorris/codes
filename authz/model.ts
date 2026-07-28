import crypto from "node:crypto";
import { generateKeys, signJwt, verifyJwt } from "./jwt.ts";

// These shapes mirror the real /authorize query params and /token form fields 1:1,
// so HTTP handlers can be layered on later without changing the model.
export type AuthorizeRequest = {
  response_type: "code";
  client_id: string;
  redirect_uri: string;
  state: string;
};

// Front channel: travels back through the resource owner's browser as a redirect.
export type AuthorizeResult = {
  redirect_uri: string;
  code: string;
  state: string;
};

export type TokenRequest = {
  grant_type: "authorization_code";
  code: string;
  redirect_uri: string;
  client_id: string;
};

// Back channel: client <-> AS directly, never through the browser.
export type TokenResponse = {
  access_token: string;
  token_type: "Bearer";
  expires_in: number;
};

// The resource owner isn't a stateful party here: the AS authenticates them out of
// band (login + consent page) and this is the outcome.
export type ResourceOwnerDecision = {
  approved: boolean;
  sub: string;
};

// Client-auth seam: a public client is `() => true`; client_secret or
// private_key_jwt verification plugs in here later without touching the flow.
export type ClientAuthenticator = (presented: unknown) => boolean;

export type Client = {
  client_id: string;
  redirect_uris: string[];
  authenticate: ClientAuthenticator;
};

const CODE_TTL_S = 60;
const ACCESS_TTL_S = 300;

// codeTtl is overridable so a harness can mint an already-expired code.
export function createAuthorizationServer(clients: Client[], { codeTtl = CODE_TTL_S } = {}) {
  const { privateKey, publicKey } = generateKeys();
  const registry = new Map(clients.map((c) => [c.client_id, c]));
  // Consumed code jtis — the one piece of state a stateless JWT cannot eliminate:
  // single-use is a property of the world, not of the token.
  const usedCodes = new Set<string>();

  function authorize(req: AuthorizeRequest, decision: ResourceOwnerDecision): AuthorizeResult {
    const client = registry.get(req.client_id);
    if (!client) throw new Error("authorize: unknown client_id");
    if (!client.redirect_uris.includes(req.redirect_uri)) {
      throw new Error("authorize: unregistered redirect_uri");
    }
    if (req.response_type !== "code") throw new Error("authorize: unsupported response_type");
    if (!decision.approved) throw new Error("authorize: resource owner denied");

    const now = Math.floor(Date.now() / 1000);
    // Binding client_id + redirect_uri into the code is what lets /token
    // validate the exchange with no lookup table.
    const code = signJwt(
      {
        typ: "code",
        sub: decision.sub,
        client_id: req.client_id,
        redirect_uri: req.redirect_uri,
        jti: crypto.randomUUID(),
        iat: now,
        exp: now + codeTtl,
      },
      privateKey,
    );
    return { redirect_uri: req.redirect_uri, code, state: req.state };
  }

  function token(req: TokenRequest, credentials?: unknown): TokenResponse {
    const client = registry.get(req.client_id);
    if (!client) throw new Error("token: unknown client_id");
    if (!client.authenticate(credentials)) throw new Error("token: client authentication failed");
    if (req.grant_type !== "authorization_code") throw new Error("token: unsupported grant_type");

    const code = verifyJwt(req.code, publicKey);
    if (code.typ !== "code") throw new Error("token: not an authorization code");
    if (code.client_id !== req.client_id) throw new Error("token: code issued to a different client");
    if (code.redirect_uri !== req.redirect_uri) throw new Error("token: redirect_uri mismatch");
    if (usedCodes.has(code.jti as string)) throw new Error("token: code already used");
    usedCodes.add(code.jti as string);

    const now = Math.floor(Date.now() / 1000);
    const access_token = signJwt(
      {
        typ: "access",
        iss: "auth-server",
        sub: code.sub,
        aud: "resource-server",
        client_id: req.client_id,
        iat: now,
        exp: now + ACCESS_TTL_S,
      },
      privateKey,
    );
    return { access_token, token_type: "Bearer", expires_in: ACCESS_TTL_S };
  }

  // publicKey is what a real AS would publish at a JWKS endpoint.
  return { publicKey, authorize, token };
}

// The RS holds only the AS public key: it can verify tokens but never mint them.
export function createResourceServer(asPublicKey: crypto.KeyObject) {
  function handleRequest(authorizationHeader: string): { data: string } {
    const [scheme, token] = authorizationHeader.split(" ");
    if (scheme !== "Bearer" || !token) throw new Error("resource: expected Bearer token");
    const claims = verifyJwt(token, asPublicKey);
    // typ/aud checks keep an authorization code from doubling as an access token.
    if (claims.typ !== "access") throw new Error("resource: not an access token");
    if (claims.aud !== "resource-server") throw new Error("resource: wrong audience");
    return { data: `hello ${claims.sub}` };
  }
  return { handleRequest };
}
