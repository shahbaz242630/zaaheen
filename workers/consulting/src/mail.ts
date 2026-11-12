// Telling consulting@ about an enquiry (ADR-121, ADR-SEC-057 §5): Microsoft
// Graph sendMail with an app-only token. The app registration holds only
// Mail.Send, and an Exchange Application Access Policy limits it to the
// consulting@ mailbox, so these credentials can send as nobody else.
// Plain text, a fixed subject, JSON fields only: nothing typed by the
// visitor reaches a mail header. Failures log a reason and a status code,
// never the enquiry.
import type { Fetcher } from "./gate";
import type { Enquiry } from "./validate";

export const MAILBOX = "consulting@zaaheen.com";
export const GRAPH_SEND = `https://graph.microsoft.com/v1.0/users/${MAILBOX}/sendMail`;
export const SUBJECT = "New enquiry: AI Opportunity Assessment";
/** Emails a day at most; enquiries above it are stored and read from the database. */
export const MAIL_DAILY_CAP = 50;

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

export function mailText(id: number, e: Enquiry, receivedAt: number): string {
  return [
    `Enquiry #${id}, received ${new Date(receivedAt).toISOString()}`,
    "",
    `Name: ${e.name}`,
    `Company: ${e.company}`,
    `Email: ${e.email}`,
    `Phone: ${e.phone}`,
    `Country: ${COUNTRY_NAMES[e.country]}`,
    `Company size: ${e.size} staff`,
    `Departments: ${e.departments.length ? e.departments.join(", ") : "not stated"}`,
    `Updates by email: ${e.marketingConsent ? "yes" : "no"}`,
    "",
    "Message:",
    e.message || "(none)",
    "",
    "They agreed to be contacted about this enquiry. Reply to this email to answer them.",
  ].join("\n");
}

/** True when Microsoft accepted the email. Never throws. */
export async function sendEnquiryMail(env: MailEnv, fetcher: Fetcher, id: number, e: Enquiry, receivedAt: number): Promise<boolean> {
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
          subject: SUBJECT,
          body: { contentType: "Text", content: mailText(id, e, receivedAt) },
          toRecipients: [{ emailAddress: { address: MAILBOX } }],
          replyTo: [{ emailAddress: { address: e.email } }],
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
