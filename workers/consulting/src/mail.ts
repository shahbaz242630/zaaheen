// Telling consulting@ about an enquiry (ADR-121, ADR-SEC-057 §5): Microsoft
// Graph sendMail with an app-only token. The app holds Mail.Send only through
// Exchange RBAC for Applications, scoped to the consulting@ mailbox (s129:
// the replacement for Application Access Policies, as the booking app uses),
// so these credentials can send as nobody else.
// Plain text, fixed subjects, JSON fields only: nothing typed by the visitor
// reaches a mail header, and the visitor's message is quoted line by line
// between fixed markers, so it cannot pose as our own lines. Failures log a
// reason and a status code, never the enquiry. Alerts carry counts only.
import type { Fetcher } from "./gate";
import type { Enquiry } from "./validate";

export const MAILBOX = "consulting@zaaheen.com";
export const GRAPH_SEND = `https://graph.microsoft.com/v1.0/users/${MAILBOX}/sendMail`;
export const SUBJECT = "New enquiry: AI Readiness Assessment";
export const CAP_SUBJECT = "Enquiry emails paused for today";
export const NIGHTLY_SUBJECT = "Enquiries waiting in the database";
/** Enquiry emails a day at most; enquiries above it are stored and read from the database. */
export const MAIL_DAILY_CAP = 50;

export const MESSAGE_START = "----- Message typed by the visitor (each line starts with >) -----";
export const MESSAGE_END = "----- End of the visitor's message -----";

export const tokenUrl = (tenant: string) =>
  `https://login.microsoftonline.com/${encodeURIComponent(tenant)}/oauth2/v2.0/token`;

export interface MailEnv {
  MS_TENANT_ID: string;
  MS_CLIENT_ID: string;
  MS_CLIENT_SECRET: string;
}

const COUNTRY_NAMES: Record<Enquiry["country"], string> = {
  AE: "United Arab Emirates",
  SA: "Saudi Arabia",
  QA: "Qatar",
  KW: "Kuwait",
  BH: "Bahrain",
  OM: "Oman",
  other: "Other",
};

// The page's own words for each choice (public/index.html; test/page.test.ts).
const TURNOVER_NAMES: Record<Enquiry["turnover"], string> = {
  "under-5m": "Under AED 5 million",
  "5m-20m": "AED 5 to 20 million",
  "20m-50m": "AED 20 to 50 million",
  "50m-200m": "AED 50 to 200 million",
  "200m+": "More than AED 200 million",
};
const SECTOR_NAMES: Record<Enquiry["sector"], string> = {
  retail: "Retail and e-commerce",
  trading: "Trading and distribution",
  manufacturing: "Manufacturing",
  "construction-real-estate": "Construction and real estate",
  "hospitality-food": "Hospitality, food and beverage",
  healthcare: "Healthcare",
  education: "Education",
  logistics: "Logistics and transport",
  "financial-services": "Banking, finance and insurance",
  "professional-services": "Professional services",
  technology: "Technology and telecoms",
  energy: "Oil, gas and energy",
  government: "Government and semi-government",
  other: "Other",
};
const CALL_TIME_NAMES: Record<Enquiry["callTime"], string> = {
  "": "no preference",
  morning: "Morning, 9 to 12",
  midday: "Midday, 12 to 3",
  afternoon: "Afternoon, 3 to 6",
  evening: "Evening, 6 to 8",
};

export function mailText(id: number, e: Enquiry, receivedAt: number): string {
  return [
    `Enquiry #${id}, received ${new Date(receivedAt).toISOString()}`,
    "They agreed to be contacted about this enquiry. Reply to this email to answer them.",
    "",
    `Name: ${e.name}`,
    `Company: ${e.company}`,
    `Email: ${e.email}`,
    `Phone: ${e.phone}`,
    `Best time to call (Gulf time): ${CALL_TIME_NAMES[e.callTime]}`,
    `Country: ${COUNTRY_NAMES[e.country]}`,
    `Number of staff: ${e.size}`,
    `Annual turnover: ${TURNOVER_NAMES[e.turnover]}`,
    `Sector: ${SECTOR_NAMES[e.sector]}`,
    `Updates by email: ${e.marketingConsent ? "yes" : "no"}`,
    "",
    MESSAGE_START,
    ...(e.message ? e.message.split("\n").map((l) => `> ${l}`) : ["> (none)"]),
    MESSAGE_END,
  ].join("\n");
}

export function capAlertText(cap: number): string {
  return [
    `Today's limit of ${cap} enquiry emails has been reached.`,
    "Further enquiries today are stored in the enquiry database but not emailed.",
    "Read them there, and check whether the form is being flooded.",
  ].join("\n");
}

export function nightlyAlertText(unmailed: number, storedYesterday: number, storedCap: number): string {
  const lines = [`Enquiries in the last 24 hours that were not emailed: ${unmailed}. Read them in the enquiry database.`];
  if (storedYesterday >= storedCap) lines.push(`Yesterday the form reached its limit of ${storedCap} enquiries; later visitors were asked to email us.`);
  lines.push("A failed email often means the mail credential has expired: check its expiry date.");
  return lines.join("\n");
}

/** One plain-text email to consulting@. True when Microsoft accepted it. Never throws. */
export async function sendMail(env: MailEnv, fetcher: Fetcher, subject: string, text: string, replyTo?: string): Promise<boolean> {
  try {
    const tokenResponse = await fetcher(tokenUrl(env.MS_TENANT_ID), {
      method: "POST",
      body: new URLSearchParams({
        grant_type: "client_credentials",
        client_id: env.MS_CLIENT_ID,
        client_secret: env.MS_CLIENT_SECRET,
        scope: "https://graph.microsoft.com/.default",
      }),
    });
    if (!tokenResponse.ok) {
      console.warn("consulting_mail_failed", "token", tokenResponse.status);
      return false;
    }
    const token: unknown = await tokenResponse.json();
    const access = typeof token === "object" && token !== null ? (token as Record<string, unknown>).access_token : undefined;
    if (typeof access !== "string" || access === "") {
      console.warn("consulting_mail_failed", "token", "no_access_token");
      return false;
    }

    const sendResponse = await fetcher(GRAPH_SEND, {
      method: "POST",
      headers: { Authorization: `Bearer ${access}`, "Content-Type": "application/json" },
      body: JSON.stringify({
        message: {
          subject,
          body: { contentType: "Text", content: text },
          toRecipients: [{ emailAddress: { address: MAILBOX } }],
          ...(replyTo ? { replyTo: [{ emailAddress: { address: replyTo } }] } : {}),
        },
        saveToSentItems: false,
      }),
    });
    if (sendResponse.status !== 202) {
      console.warn("consulting_mail_failed", "send", sendResponse.status);
      return false;
    }
    return true;
  } catch {
    console.warn("consulting_mail_failed", "network");
    return false;
  }
}

export function sendEnquiryMail(env: MailEnv, fetcher: Fetcher, id: number, e: Enquiry, receivedAt: number): Promise<boolean> {
  return sendMail(env, fetcher, SUBJECT, mailText(id, e, receivedAt), e.email);
}
