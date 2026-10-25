# Every message Zaaheen can send you

When Zaaheen cannot do what you asked, it answers with a message instead of a result. Pass it on to the user in plain words and do what it says. Quoted text is exact.

## Saving into an area that does not exist

Starts with **"Not saved: there is no area called"**, then names the user's areas, a close match if there is one ("Did you mean …?"), and the steps to create an area. Nothing was saved. Save into the area the user meant, or walk them through creating it (topic `areas`) and save again once they confirm.

## Deletes and edits paused (the hourly limit)

> "Zaaheen has paused deletes and edits from AI apps for a while, to protect your memories. You can delete or edit them in the Zaaheen app."

AI apps together can delete or change 30 memories an hour. Nothing changed. Tell the user nothing is wrong, the pause lifts within the hour, and they can delete or edit more now themselves in the app (topic `memories`). Reading and saving still work.

## Busy

> "Zaaheen is still answering other questions and answers one at a time. Ask again in a moment, and ask one question at a time, waiting for each answer."

Wait a moment and ask again, one question at a time. Do not tell the user it failed.

## Starting, timing out, restarting

- > "the vault is starting; try again in a moment" (wait a few seconds and try again)
- > "the vault is busy (maintenance or another program is using it); try again shortly" (the tidy-up or a move may be running; try again shortly)
- > "the vault took too long to answer; try again" (a read, search or delete; repeat it)
- > "the vault took too long to answer; the save may still complete, so check with memory_read before saving again" (do not save twice: check first)
- > "the vault restarted while handling this request; if it was a save, check with memory_read before saving again"

## Needs the user to act in the Zaaheen app

- > "the vault could not start; open the Zaaheen app to check it" (ask the user to open the app; it shows what is wrong. If it persists, topic `troubleshooting`)
- > "Zaaheen can't find your memories; open the Zaaheen app to check where they are" (usually the memories are on a drive that is unplugged, or a different drive took its letter; plug it back in, or check Settings, Your memories)
- > "Zaaheen was updated; restart every app that uses Zaaheen" (quit this AI app completely and open it again)
- > "the vault's key changed; restart every app that uses Zaaheen" (same: restart the AI apps)
- > "the vault refused this connection" (restart the AI app; if it continues, remove and add Zaaheen again, topic `connect`)

## Account and subscription (Zaaheen is locked)

Nothing is served until the user acts. Nothing has been deleted.
- > "Zaaheen needs you to sign in. Open the Zaaheen app on this computer to sign in."
- > "Zaaheen couldn't confirm your subscription. Check your internet connection and try again."
- > "Your Zaaheen free trial has ended. Open the Zaaheen app and choose Subscribe to keep using your memories. Nothing has been deleted."
- > "Your Zaaheen subscription has ended. Open the Zaaheen app and choose Subscribe to keep using your memories. Nothing has been deleted."
- > "Zaaheen is unlocking. Try again in a moment." (the user just signed in or paid; try again shortly)

Topic `account` has the steps.

## Notices inside a memory_read answer (health.warnings)

These ride on a normal answer; the facts are still usable.
- **SUBSCRIPTION_TRIAL_ENDING** ("Your Zaaheen free trial ends in N days." / "in less than a day.", with "Open the Zaaheen app and choose Subscribe to keep using your memories.") and **SUBSCRIPTION_PAYMENT_FAILED** ("Your last Zaaheen payment didn't go through." with "Open the Zaaheen app to update your card."): tell the user once per conversation in one short sentence, then answer normally.
- **REPORT_MISSING**: the memories have not been tidied yet (normal on a new install); facts are still found.
- **REPORT_STALE_INFO**, **REPORT_STALE_WARN**, **REPORT_STALE_CRITICAL**: the last tidy-up was a day, three days or a week or more ago. With `status` "critical", tell the user results may be incomplete and suggest Consolidation, **Run consolidation now** (topic `consolidation`).
- **TOPIC_NAMES_UNAVAILABLE**: topic labels are placeholders; ignore the `topic` names.
- **CLOCK_SKEW_DETECTED**: the computer's clock looks wrong; suggest setting it to update automatically. (The app also shows "Your computer's clock is wrong. Set it to update automatically.")

## Generic errors

Short protocol errors carry no detail on purpose: "access denied", "invalid params" (a value you sent was not accepted, for example an unknown memory type, a malformed id or date), "not found" (no memory with that id; it may have been deleted), "internal error". Check what you sent and try once more; if it repeats, tell the user and suggest topic `troubleshooting`.
