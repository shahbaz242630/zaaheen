# Areas

## What areas are

- Memories are kept in **areas**. The Zaaheen app calls them **boundaries**: its **Boundaries** tab lists them. Every memory is in exactly one area.
- Areas are for organising memories (for example Health, work, family). **Every AI app the user connects can read and save in every area**, including an area the user creates later: it is available to you straight away, with no restart.
- Two areas always exist:
  - **default**: what the user adds in the Zaaheen app when they choose it (the app labels it "What you add in this app").
  - **personal**: where AI apps save when the user does not name an area (labelled "What your AI apps save").
- The Boundaries tab shows each area with its label and how many memories it holds.
- There is no limit on how many areas the user can have.

## Which area to save into

1. If the user names an area ("save this in my Health area"), use that area's exact name.
2. If they do not, use **personal**.
3. Names are exact: capitals matter. "Health" and "health" are different names, and the user cannot have both (Zaaheen refuses a new area whose name differs from an existing one only by capitals). If the user says "health" and their area is "Health", use "Health".
4. Call `memory_help` (no topic) to see the user's areas right now, with their labels and counts.

## Only the user creates areas

You cannot create an area. If you save into an area that does not exist, **nothing is saved**, and Zaaheen answers with the user's areas, a close match if there is one (for example "Did you mean Health?"), and the steps below. Then either save into the area the user meant, or walk them through creating it and save again once they confirm.

### How the user creates an area

1. Open the Zaaheen app.
2. Click **Boundaries** at the top.
3. Click **+ New boundary**.
4. Type the name: letters, numbers, - or _ only, no spaces, up to 64 characters (for example Health or work-projects).
5. Optionally type a short label in the second box (for example "Doctor visits and medicines"). The label only describes the area; it is not a memory, and nothing typed there is saved as a memory.
6. Click **Create boundary**.

If the name is already taken (capitals ignored), the app says so; use the existing area. A new area starts empty.

### How the user puts memories into an area

- In the app: **Memories** tab, **+ Add a memory**, type it, choose what it is, choose the **Area** from the list (saving stays off until an area is chosen), then **Keep this memory**. A new area appears in the list as soon as it is created.
- Through you: the user asks you to save something into that area, and you save it there with `memory_write`.

## Moving a memory to another area

Use `memory_update` with the memory's id and the new area's exact name (keep the wording). The area must already exist.

## Memories from earlier versions

In earlier versions of Zaaheen, memories the user typed into the app went into **default**, and AI apps could read only **personal**. Now every app reads every area, so those memories are visible too; nothing needs moving.

## When something seems missing

Never say an area or a memory does not exist without checking. Search with other words, call `memory_help` to see the areas and counts, and see topic `troubleshooting` ("A memory seems missing").
