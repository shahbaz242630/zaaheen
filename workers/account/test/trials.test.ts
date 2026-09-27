// One free trial per email, even after deleting (ACCOUNT-DELETION-DESIGN D8):
// the normalised email, its HMAC fingerprint, and the first-lease lookup
// against a fake TRIALS namespace.
import { describe, expect, it } from "vitest";

import { DAY } from "../src/time";
import { TRIAL_MEMORY_SECONDS, TrialError, firstTrialStart, normaliseEmail, trialFingerprint } from "../src/trials";
import { fakeKv } from "./support";

const T = 1_800_000_000;
const KEY = "k".repeat(32);
const iso = (epoch: number) => new Date(epoch * 1000).toISOString();

describe("normaliseEmail", () => {
  const cases: Array<[string, string]> = [
    ["a@example.com", "a@example.com"],
    ["  A@Example.COM \n", "a@example.com"],
    ["a+news@example.com", "a@example.com"],
    ["a+one+two@example.com", "a@example.com"],
    ["a.b@example.com", "a.b@example.com"],
    ["a.b+x@gmail.com", "ab@gmail.com"],
    ["A.B@GoogleMail.com", "ab@gmail.com"],
    ["a.b@googlemail.com", "ab@gmail.com"],
    ["a.b@notgmail.com", "a.b@notgmail.com"],
    ["a.b@gmail.com.example", "a.b@gmail.com.example"],
    // A leading + is the whole name, not a tag (session 68 review NIT).
    ["+x@example.com", "+x@example.com"],
    ["+x+y@example.com", "+x@example.com"],
  ];
  for (const [input, want] of cases) {
    it(`${JSON.stringify(input)} -> ${want}`, () => {
      expect(normaliseEmail(input)).toBe(want);
    });
  }

  it("only the local part loses a +tag", () => {
    expect(normaliseEmail("a@ex+ample.com")).toBe("a@ex+ample.com");
  });
});

describe("trialFingerprint", () => {
  it("is 64 lower-case hex characters, HMAC-SHA256 with TRIAL_KEY", async () => {
    const fp = await trialFingerprint(KEY, "a@example.com");
    expect(fp).toMatch(/^[0-9a-f]{64}$/);
    const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(KEY), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode("a@example.com")));
    expect(fp).toBe(Array.from(mac, (b) => b.toString(16).padStart(2, "0")).join(""));
  });

  it("is the same for every spelling of one address, and differs by key and address", async () => {
    const base = await trialFingerprint(KEY, "ab@gmail.com");
    for (const variant of ["a.b@gmail.com", "A.B+x@googlemail.com", " ab@GMAIL.com "]) {
      expect(await trialFingerprint(KEY, variant)).toBe(base);
    }
    expect(await trialFingerprint("j".repeat(32), "ab@gmail.com")).not.toBe(base);
    expect(await trialFingerprint(KEY, "ac@gmail.com")).not.toBe(base);
  });
});

describe("firstTrialStart", () => {
  it("not found: starts now and stores the start for two years", async () => {
    const kv = fakeKv();
    expect(await firstTrialStart(kv.store, KEY, "a@example.com", T)).toBe(T);
    expect(kv.puts).toEqual([{ key: await trialFingerprint(KEY, "a@example.com"), value: iso(T), options: { expirationTtl: TRIAL_MEMORY_SECONDS } }]);
    expect(TRIAL_MEMORY_SECONDS).toBe(730 * DAY);
  });

  it("found: the earlier start, and nothing is written", async () => {
    const kv = fakeKv();
    kv.data.set(await trialFingerprint(KEY, "a@example.com"), iso(T - 100 * DAY));
    expect(await firstTrialStart(kv.store, KEY, "a+again@example.com", T)).toBe(T - 100 * DAY);
    expect(kv.puts).toEqual([]);
  });

  it("a stored start in the future never lengthens a trial", async () => {
    const kv = fakeKv();
    kv.data.set(await trialFingerprint(KEY, "a@example.com"), iso(T + DAY));
    expect(await firstTrialStart(kv.store, KEY, "a@example.com", T)).toBe(T);
  });

  it("fails closed: no store, no email, a store error or an unreadable value throws", async () => {
    await expect(firstTrialStart(undefined, KEY, "a@example.com", T)).rejects.toBeInstanceOf(TrialError);
    await expect(firstTrialStart(fakeKv().store, KEY, null, T)).rejects.toBeInstanceOf(TrialError);
    await expect(firstTrialStart(fakeKv("get").store, KEY, "a@example.com", T)).rejects.toBeInstanceOf(TrialError);
    await expect(firstTrialStart(fakeKv("put").store, KEY, "a@example.com", T)).rejects.toBeInstanceOf(TrialError);
    const kv = fakeKv();
    kv.data.set(await trialFingerprint(KEY, "a@example.com"), "not a time");
    await expect(firstTrialStart(kv.store, KEY, "a@example.com", T)).rejects.toBeInstanceOf(TrialError);
  });

  it("its errors never carry the email or the fingerprint", async () => {
    const fp = await trialFingerprint(KEY, "a@example.com");
    const kv = fakeKv();
    kv.data.set(fp, "not a time");
    for (const attempt of [
      firstTrialStart(kv.store, KEY, "a@example.com", T),
      firstTrialStart(fakeKv("get").store, KEY, "a@example.com", T),
      firstTrialStart(fakeKv("put").store, KEY, "a@example.com", T),
    ]) {
      const e = await attempt.catch((x: unknown) => x);
      expect(String((e as Error).message)).not.toContain("example.com");
      expect(String((e as Error).message)).not.toContain(fp);
    }
  });
});
