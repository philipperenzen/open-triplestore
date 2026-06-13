// Browser glue for WebAuthn passkeys.
//
// The server (webauthn-rs) speaks the standard WebAuthn JSON encoding where
// every binary field is a base64url string, while navigator.credentials wants
// ArrayBuffers. These helpers translate both directions explicitly instead of
// relying on PublicKeyCredential.parseCreationOptionsFromJSON / toJSON, which
// older Safari/Firefox releases lack.

function b64urlToBuf(s) {
  const pad = '='.repeat((4 - (s.length % 4)) % 4);
  const bin = atob(s.replace(/-/g, '+').replace(/_/g, '/') + pad);
  const buf = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
  return buf.buffer;
}

function bufToB64url(buf) {
  const bytes = new Uint8Array(buf);
  let bin = '';
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** True when the browser exposes the WebAuthn API. */
export function isPasskeySupported() {
  return typeof window !== 'undefined'
    && !!window.PublicKeyCredential
    && !!navigator.credentials?.create;
}

/**
 * Run navigator.credentials.create() from the server's creation options
 * (the `options.publicKey` member of /api/auth/passkeys/register/start) and
 * return the JSON-encoded credential for register/finish.
 */
export async function createPasskey(publicKey) {
  const options = {
    ...publicKey,
    challenge: b64urlToBuf(publicKey.challenge),
    user: { ...publicKey.user, id: b64urlToBuf(publicKey.user.id) },
    excludeCredentials: (publicKey.excludeCredentials || []).map((c) => ({
      ...c,
      id: b64urlToBuf(c.id),
    })),
  };
  const cred = await navigator.credentials.create({ publicKey: options });
  return {
    id: cred.id,
    rawId: bufToB64url(cred.rawId),
    type: cred.type,
    extensions: cred.getClientExtensionResults?.() ?? {},
    response: {
      attestationObject: bufToB64url(cred.response.attestationObject),
      clientDataJSON: bufToB64url(cred.response.clientDataJSON),
      transports: cred.response.getTransports?.() ?? undefined,
    },
  };
}

/**
 * Run navigator.credentials.get() from the server's request options
 * (the `options.publicKey` member of /api/auth/passkeys/login/start) and
 * return the JSON-encoded assertion for login/finish.
 */
export async function getPasskeyAssertion(publicKey) {
  const options = {
    ...publicKey,
    challenge: b64urlToBuf(publicKey.challenge),
    allowCredentials: (publicKey.allowCredentials || []).map((c) => ({
      ...c,
      id: b64urlToBuf(c.id),
    })),
  };
  const cred = await navigator.credentials.get({ publicKey: options });
  return {
    id: cred.id,
    rawId: bufToB64url(cred.rawId),
    type: cred.type,
    extensions: cred.getClientExtensionResults?.() ?? {},
    response: {
      authenticatorData: bufToB64url(cred.response.authenticatorData),
      clientDataJSON: bufToB64url(cred.response.clientDataJSON),
      signature: bufToB64url(cred.response.signature),
      userHandle: cred.response.userHandle ? bufToB64url(cred.response.userHandle) : null,
    },
  };
}
