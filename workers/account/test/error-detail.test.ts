// What a failure may say in a log line (audit s104 API-6, s101 W-08): our own
// errors keep their fixed wording; any other error logs only its kind, since
// its message may carry a value from a request or a reply.
import { describe, expect, it } from "vitest";

import { BillingError } from "../src/billing";
import { errorDetail } from "../src/routes/common";
import { TrialError } from "../src/trials";
import { UpstreamError } from "../src/upstream";

describe("errorDetail", () => {
  it("keeps the wording of our own errors", () => {
    expect(errorDetail(new UpstreamError("paddle get subscription: status 502"))).toBe("paddle get subscription: status 502");
    expect(errorDetail(new BillingError("a subscription is not an object"))).toBe("a subscription is not an object");
    expect(errorDetail(new TrialError("trial store unavailable"))).toBe("trial store unavailable");
  });

  it("logs only the kind of any other error, never its message", () => {
    expect(errorDetail(new TypeError("Cannot read properties of someone@example.com"))).toBe("TypeError");
    expect(errorDetail(new Error("token abc.def.ghi"))).toBe("Error");
    expect(errorDetail("a thrown string")).toBeUndefined();
  });
});
