Your task is to document this repository in the knowledge base, so that agents working in other repositories can find how it works without reading its code.

What the user typed after the command (may be empty): $ARGUMENTS

- Words there are the focus: plan only the topics about them, and leave the rest of the repository alone.
- `--domain <id>` there names the domain to file the documents under. Use it for every document unless one plainly belongs to another domain, and say which ones did. Without it, choose each document's domain from the descriptions of the domains under "Knowledge"; add a domain with `newDomains` only when none fits.

The "Knowledge" section of your instructions lists what is already written about this repository. Documents marked stale or no evidence are to be checked and written again; the others are done — do not write them again unless the focus asks for it.

## Step 1. Plan

Find the topics without reading the code end to end: the layout of the tracked files, the README, the build manifest and the configuration (`application.yml`, `.env.example`, Helm values and the like), then the entry points — controllers and routes, message producers and consumers, scheduled jobs, the database access layer.

Write the plan with `todo`, one task per topic and kind — at most 20 tasks, `service` included; when there are more topics, merge the small ones of a kind into one task (one document may still cover each):

- `service` — one per repository: what it is for, what it talks to. Always the last task.
- `api` — one per group of endpoints that belong together.
- `event` — one per topic or queue it publishes or reads.
- `datastore` — one per table or collection it owns.
- `algorithm` — one per calculation that is not obvious from its name: inputs, steps, edge cases.
- `concept` — a term or a rule of the domain the code depends on.
- `howto` — a task people do often: add an endpoint, run it locally, change a setting.

Skip topics that are already written and not marked. A written document whose topic is gone from the code goes in the plan as a delete.

## Step 2. Ask

For each topic, send `explore` with one question about it — what a document on that topic has to say, and that every fact needs its file and lines. The brief's first line is that question in a short sentence: the app shows it as the helper's name. Do not name the repository or its path — the helper works in the same folder. Topics that do not depend on each other go out together, several `explore` calls in one response. A helper answers from the code and writes nothing.

When an answer is thin or contradicts another, ask again or read the code yourself before writing anything on it.

## Step 3. Write

One topic at a time: its helper's answer becomes a document, and `knowledgeWrite` keeps it. Follow the format and the rules in that tool's description. Only facts with evidence you saw are `confirmed`; say `inferred` or `unknown` rather than guess. When a document is refused, fix the reasons it gives and send it again; do not reword it to get past the check.

Write the `service` overview last, from the documents already written — read them with `knowledgeRead` — not from the code again.

## Step 4. Report

End with a short report: the documents written, rewritten and deleted, by id; the domains they went to, and any domain you added; what was refused and why; and the topics you left out or could not settle.
