# Consolidation (the automatic tidy-up)

## What it does

While the user is away, Zaaheen tidies what it has learned, on the user's own computer:

- **Merges near-duplicates** into one memory.
- **Settles contradictions** in favour of the newer fact.
- **Lets old details fade**: memories never used move out of everyday recall (they can still be found with `memory_search` and `include_archived: true`).
- Writes short summaries of each topic and adds other ways of phrasing a memory, so AI apps find it whatever words they use.

It works only from what was saved, **never deletes a memory**, never sends anything off the computer, and keeps each area separate. Like any AI it can occasionally get something wrong, so the user should check anything important.

Facts saved since the last tidy-up are found straight away; they just have no topic yet (`topic` is empty in `memory_read`).

## The Consolidation tab

Open the Zaaheen app and click **Consolidation** at the top.

1. **Consolidate automatically**: tick or untick **Keep my vault tidy automatically**. Turning it off removes the scheduled job from the computer.
2. **When it runs**: **How often** (**Every day** or **Once a week**, and **On** a weekday for weekly), **At** a time, then **Save schedule**. Choose a time the computer is usually on but not in use; the middle of the night is ideal, since a tidy-up uses real processing power for a few minutes. The tab shows when it last ran and what it did.
3. **Run one now**: **Run consolidation now** runs it immediately (useful after adding many memories). If an AI app is using the memories at that moment, it waits and runs a little later.

Setup turns it on (every day, at a quiet hour) unless the user chose **Not now**.

## Good to know

- It runs even when the Zaaheen window is closed. On a computer that was asleep at the scheduled time, it catches up the next time the app is opened.
- The first time it is turned on, Zaaheen downloads its tidy-up engine once (a large download); the tab shows progress, and **Run consolidation now** is available when it is ready.
- On a Mac, macOS may announce the schedule as a background item. That is expected.
- If `memory_read` reports `health.status` "critical" (memories not tidied for a long time), suggest the user opens the Consolidation tab and clicks **Run consolidation now**, and checks the schedule is on.
