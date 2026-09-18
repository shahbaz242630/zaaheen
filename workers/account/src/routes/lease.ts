// POST /v1/lease (SIGNIN-DESIGN.md §5; the contract S1 speaks, §8.27):
//
//   Authorization: Bearer <opaque access token>
//   {"client_now": <i64 > 0>, "app_version": "<1-32 of 0-9A-Za-z.+->"}
//   200 {"lease": "<wire>"}   401 token refused   429/5xx try later
//
// Order: request checks (no upstream call for a bad request), the signing
// key (so a broken deploy fails before any write), Clerk verify, the user,
// the §5 decision, then the signature. The lease is signed for the subject
// Clerk verified, never for anything the client sent.

import { ClerkClient } from "../clerk";
import type { Config } from "../config";
import { decideLease } from "../decide";
import { bearerToken, errorResponse, jsonResponse } from "../http";
import { type LeasePayload, type LeaseSigningKey, importLeaseKey, signLease } from "../lease";
import { PaddleClient } from "../paddle";
import { parseRecord } from "../record";
import { OFFLINE_DAYS } from "../time";
import { UpstreamError } from "../upstream";
import { type RouteDeps, authenticate, log, readJsonObject } from "./common";

export type { RouteDeps } from "./common";

const MAX_BODY_BYTES = 1024;
const APP_VERSION = /^[0-9A-Za-z.+-]{1,32}$/;

export async function handleLease(request: Request, config: Config, deps: RouteDeps): Promise<Response> {
  if (request.method !== "POST") return errorResponse(405, "method_not_allowed", { allow: "POST" });
  const token = bearerToken(request);
  if (token === null) return errorResponse(401, "token_refused");
  const body = await readBody(request);
  if (body === null) return errorResponse(400, "bad_request");

  let signer: LeaseSigningKey;
  try {
    signer = await importLeaseKey(config.lease.kid, config.lease.pkcs8);
  } catch {
    log("lease", "signing_key_unusable");
    return errorResponse(503, "unavailable");
  }

  const clerk = new ClerkClient(config.clerk, deps.fetch);
  const paddle = new PaddleClient(config.paddle, deps.fetch);
  try {
    const auth = await authenticate(clerk, token);
    if (auth.kind === "refused") return auth.response;
    const { record, warnings } = parseRecord(auth.user.privateMetadata);
    for (const w of warnings) log("lease", "record_warning", w);

    const now = deps.now();
    const decision = await decideLease(record, {
      now,
      killSwitch: config.killSwitch,
      productId: config.paddle.productId,
      fetchSubscriptions: (customerId) => paddle.listSubscriptions(customerId),
      writeRecord: (patch) => clerk.mergeRecord(auth.sub, patch),
    });
    if (decision.kind === "unavailable") return errorResponse(503, "unavailable");

    const payload: LeasePayload = {
      v: 1,
      kid: signer.kid,
      sub: auth.sub,
      ...decision.terms,
      issued_at: now,
      client_time: body.client_now,
      offline_days: OFFLINE_DAYS,
    };
    return jsonResponse(200, { lease: await signLease(payload, signer) });
  } catch (e) {
    log("lease", e instanceof UpstreamError ? "upstream_error" : "unexpected_error", e instanceof Error ? e.message : undefined);
    return errorResponse(503, "unavailable");
  }
}

interface LeaseRequest {
  client_now: number;
  app_version: string;
}

async function readBody(request: Request): Promise<LeaseRequest | null> {
  const raw = await readJsonObject(request, MAX_BODY_BYTES);
  if (raw === null) return null;
  const { client_now, app_version } = raw;
  if (!Number.isSafeInteger(client_now) || (client_now as number) <= 0) return null;
  if (typeof app_version !== "string" || !APP_VERSION.test(app_version)) return null;
  return { client_now: client_now as number, app_version };
}
