import crypto from "node:crypto";

export type JwtPayload = Record<string, unknown>;

export function base64url(buf: Buffer): string {
  return buf.toString("base64url");
}

export function fromBase64url(s: string): Buffer {
  return Buffer.from(s, "base64url");
}

// The AS keeps privateKey; publicKey is the only thing other parties ever get.
export function generateKeys() {
  return crypto.generateKeyPairSync("ed25519");
}

const HEADER = base64url(Buffer.from(JSON.stringify({ alg: "EdDSA", typ: "JWT" })));

export function signJwt(payload: JwtPayload, privateKey: crypto.KeyObject): string {
  const body = `${HEADER}.${base64url(Buffer.from(JSON.stringify(payload)))}`;
  const sig = crypto.sign(null, Buffer.from(body), privateKey);
  return `${body}.${base64url(sig)}`;
}

// Anyone can read a JWT's claims; only signJwt/verifyJwt need key material.
export function decodeJwt(token: string): JwtPayload {
  const payload = token.split(".")[1];
  if (!payload) throw new Error("jwt: malformed");
  return JSON.parse(fromBase64url(payload).toString());
}

// Checks signature and expiry only — claim policy (typ, aud, ...) belongs to the parties.
export function verifyJwt(token: string, publicKey: crypto.KeyObject): JwtPayload {
  const [header, payload, sig] = token.split(".");
  if (!header || !payload || !sig) throw new Error("jwt: malformed");
  const ok = crypto.verify(null, Buffer.from(`${header}.${payload}`), publicKey, fromBase64url(sig));
  if (!ok) throw new Error("jwt: bad signature");
  const claims = JSON.parse(fromBase64url(payload).toString()) as JwtPayload;
  if (typeof claims.exp !== "number") throw new Error("jwt: missing exp");
  if (claims.exp <= Math.floor(Date.now() / 1000)) throw new Error("jwt: expired");
  return claims;
}
