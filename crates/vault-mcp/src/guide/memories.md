# Memories

## Saving well (memory_write)

Every connected AI app reads what you save, so save in one clear shape:

1. **One fact per memory.** Split "The user likes football and lives in Dubai" into two saves.
2. **About the user, in the third person.** "The user prefers short answers", not "I prefer short answers".
3. **Complete sentences.** Subject, verb, object; never fragments.
4. **No conversation framing.** "The user prefers Python", not "When asked, the user said Python".
5. **Real dates for things that change.** "As of 2026-10-25 the user is training for a marathon", not "currently". You can also give the date the fact became true in `as_of`.
6. **Never about yourself.** No "I learned", "I think", "I noticed".
7. **Break long documents into facts.** Do not store a whole document as one memory; save each fact on its own. Only about the first 2000 characters of a memory are used to find it.

Save things the user has said or decided, their preferences, role, projects and recurring habits. When in doubt, save it: the nightly tidy-up merges repeats. Do not save passing chat, anything the user asked you not to keep, or passwords and other secrets unless the user clearly asks.

Choose the area as described in topic `areas` (default to **personal** when the user names none). Choose the type: `semantic` for a fact about the user (the default), `episodic` for something that happened, `procedural` for how the user does things.

## Reading (memory_read and memory_search)

- Ask `memory_read` one natural question per call. Read `relevant_facts` even when `abstain` is true: they are the closest matches for you to judge. If none answers the question, say so plainly ("I don't have your favourite city saved") and offer to save it.
- If two facts conflict about something that has one value (where the user lives, their job), the one with the newer `as_of`, or wording like "now" or "moved to", is current.
- `memory_search` returns a ranked list. If `weak_match` is true, nothing really matched; do not keep rephrasing the same search.

## Memory types in the app

The app shows each memory's type at the start of its line: **fact** (a fact about me), **event** (something that happened), **how-to** (how I do things).

## Finding memories in the app

1. Open the Zaaheen app, on the **Memories** tab. It lists the most recent memories under **Recently remembered**.
2. To search, type in the box "Search your memories, then press Enter" and press Enter (or press **Ctrl K** anywhere to jump to the box). The list title changes to **Recalled**.

## Adding a memory in the app

1. **Memories** tab, click **+ Add a memory**.
2. Type the memory in the box ("Something worth remembering…").
3. Choose what it is: **a fact about me**, **something that happened**, or **how I do things**.
4. Choose the **Area**. Saving stays off until one is chosen.
5. Click **Keep this memory** (or **cancel**).

## Changing a memory

The app has no edit button. To change a memory, you change it with `memory_update` (its id comes from `memory_read` or `memory_search`), or the user forgets it in the app and adds the new wording. Change a memory only when the user asks or a fact clearly changed; to add something new, save a new memory instead. The old wording waits in Recently deleted for 30 days.

## Deleting a memory

- **Through you:** `memory_delete` with its id, only when the user asks you to forget something. Deleting an id that is already gone counts as done.
- **In the app:** click **forget** at the end of the memory's line, then **Forget it**. It leaves the memories straight away and no AI app can see it.

## Recently deleted (undo, 30 days)

Every deleted memory, and the old wording of every changed one, waits in **Recently deleted** for 30 days, then is erased for good. Only the user can see it there; AI apps cannot.

1. Open the Zaaheen app, on the **Memories** tab.
2. Click **Recently deleted** (above the list). Each entry says whether it was deleted or replaced, and by whom (you or an AI app).
3. Click **restore** to put it back, or **delete now** to erase it straight away.
4. **Empty Recently deleted** erases them all at once.

## The hourly limit

AI apps together can delete or change 30 memories an hour. After that, deletes and edits from AI apps are paused for the rest of the hour and you receive a message saying so (topic `messages`). Reading and saving are never limited. The user can still delete in the app.
