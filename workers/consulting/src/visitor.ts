// Who is sending, without keeping who (ADR-SEC-057 §3): the same daily keyed
// hash as Ask Zaaheen (workers/ask/src/budget.ts; copied, not shared, because
// each Worker deploys on its own). Only the per-visitor daily count uses it.

/**
 * HMAC-SHA256 of the visitor's network (visitorAddress) and the day under a
 * Worker secret, as hex. Changes every day, so nothing links a visitor
 * across days, and a leaked count table cannot be matched to addresses
 * without the secret.
 */
export async function visitorKey(address: string, day: string, secret: string): Promise<string> {
  const encoder = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", encoder.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  const mac = await crypto.subtle.sign("HMAC", key, encoder.encode(`${day}\n${visitorAddress(address)}`));
  return [...new Uint8Array(mac)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/**
 * The part of an address that is one visitor. IPv4: the whole address. IPv6:
 * the first 64 bits, because one home or phone network is given a whole /64,
 * and counting single IPv6 addresses would give one person billions of
 * visitors' limits. Anything unrecognised is used as it is.
 */
export function visitorAddress(address: string): string {
  const mapped = /^::ffff:(\d{1,3}(?:\.\d{1,3}){3})$/i.exec(address);
  if (mapped) return mapped[1] ?? address;
  if (!address.includes(":")) return address;
  const groups = ipv6Groups(address);
  return groups ? `${groups.slice(0, 4).join(":")}::/64` : address;
}

/** The eight groups of an IPv6 address, lower case without leading zeros; null if malformed. */
function ipv6Groups(address: string): string[] | null {
  const halves = address.toLowerCase().split("::");
  if (halves.length > 2) return null;
  const left = halves[0] ? halves[0].split(":") : [];
  const right = halves.length === 2 && halves[1] ? halves[1].split(":") : [];
  const missing = 8 - left.length - right.length;
  if (halves.length === 2 ? missing < 1 : missing !== 0) return null;
  const groups = [...left, ...Array<string>(halves.length === 2 ? missing : 0).fill("0"), ...right];
  if (!groups.every((group) => /^[0-9a-f]{1,4}$/.test(group))) return null;
  return groups.map((group) => group.replace(/^0+(?=.)/, ""));
}
