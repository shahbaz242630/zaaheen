// Time in the account Worker: whole epoch seconds, server clock only.

export const MINUTE = 60;
export const HOUR = 60 * MINUTE;
export const DAY = 24 * HOUR;

/** The free trial (BRD §1.6 amendment 1). */
export const TRIAL_SECONDS = 30 * DAY;
/** The offline allowance every lease grants (§4, locked). */
export const OFFLINE_DAYS = 30;

const RFC3339 = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(\.\d+)?(Z|[+-]\d{2}:\d{2})$/;

/**
 * An RFC 3339 timestamp (what Paddle sends) as whole epoch seconds, rounded
 * down. Throws on anything else, including dates that do not exist.
 */
export function epochSeconds(value: unknown): number {
  const m = typeof value === "string" ? RFC3339.exec(value) : null;
  if (!m) throw new RangeError("not an RFC 3339 timestamp");
  const [year, month, day, hour, minute, second] = m.slice(1, 7).map(Number) as [
    number, number, number, number, number, number,
  ];
  const utc = Date.UTC(year, month - 1, day, hour, minute, second);
  const check = new Date(utc);
  if (
    check.getUTCFullYear() !== year ||
    check.getUTCMonth() !== month - 1 ||
    check.getUTCDate() !== day ||
    hour > 23 ||
    minute > 59 ||
    second > 59
  ) {
    throw new RangeError("not a real date and time");
  }
  const zone = m[8] ?? "Z";
  const offset = zone === "Z" ? 0 : (zone.startsWith("-") ? -1 : 1) * (Number(zone.slice(1, 3)) * HOUR + Number(zone.slice(4, 6)) * MINUTE);
  return utc / 1000 - offset;
}

/**
 * A `YYYY-MM-DD` date as the epoch second just after that day ends (UTC):
 * "free until 2026-12-31" includes the 31st. Throws on anything else.
 */
export function endOfDaySeconds(value: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!m) throw new RangeError("not a YYYY-MM-DD date");
  const [year, month, day] = m.slice(1, 4).map(Number) as [number, number, number];
  const start = new Date(Date.UTC(year, month - 1, day));
  if (start.getUTCFullYear() !== year || start.getUTCMonth() !== month - 1 || start.getUTCDate() !== day) {
    throw new RangeError("not a real date");
  }
  return start.getTime() / 1000 + DAY;
}
