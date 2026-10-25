# Privacy and security

## On the computer

- **Memories never leave the user's computer.** They are encrypted on disk, and so is everything Zaaheen builds from them (its search index and the summaries it writes when tidying). Someone who copied the folder would see only scrambled data.
- **The key** that unlocks them is created on the computer and kept in the computer's own secure store for the user's account: Windows Credential Manager on Windows, the login keychain on a Mac. It is never uploaded, never sent to the company, and never written into the memory folder.
- **Zaaheen's AI runs on the computer.** Finding the right memories, ranking them and tidying them all happen locally, and work offline once downloaded. Memories are never sent to an online AI service by Zaaheen. Zaaheen does not chat or write answers: it hands back the memories that match, as stored, and the AI app writes the reply.
- **Only apps on the same computer can connect.** AI apps talk to Zaaheen through a private channel inside the computer. Zaaheen refuses connections from other computers and opens nothing to the internet, and each app must introduce itself before Zaaheen answers.
- Zaaheen cannot protect the memories from harmful software already running on the computer, or from someone who can sign in to the user's account on it. Keep Windows or macOS up to date and the account protected with a password or PIN.

## What the company can and cannot see

- The company **cannot read the memories**: they never reach it, so a break-in there could not expose them.
- **The account service** knows who the user is, when the trial started, whether they have paid, and choices made at sign-up. Zaaheen checks the subscription with it when it starts and once a day; only the subscription is checked, never the memories. Signing in happens in the browser, so the app never sees the password; it keeps a sign-in token in the computer's secure store, used only to check the subscription. Signing in has nothing to do with the key to the memories.
- **Payments** are handled by the company's payment provider on its secure checkout page; card details never pass through the Zaaheen app.
- Memories are **never used to train AI**, by the company or anyone else.
- Everything between the app, the websites and the account service travels encrypted.

## The AI apps the user connects

They are made by other companies. When one uses a memory to answer, it may send that memory to its maker's servers as part of the conversation. What happens next, including whether conversations train their models, depends on that company's own policy and the user's settings there.

## Who can see what inside Zaaheen

- Every AI app the user connects can read and save in every area (topic `areas`).
- Everything apps read or write is recorded in a local audit log on the computer (Settings, About Zaaheen shows whether its history is verified).
- **Recently deleted** is visible only to the user, in the app; AI apps cannot see or restore from it.

## Deleting

- **Delete everything** destroys the key first, then the memory files, so any encrypted copy left on the disk or in a backup can never be unlocked again. Files saved with Download my memories are readable copies and are not affected; the user deletes them.
- **Delete my account** deletes the memories on this computer and the account (topic `account`).

## Where the full policies are

Settings, **Documents**, **Policies**: Privacy Policy, AI and Your Data, Security, Terms of Service.
