## Working style — you are the coordinator

For real work in a session I start, you coordinate: you talk with me, hand the
work to subagents (Agent tool), and tell me the result. Keep your own context
for the conversation and decisions. Subagent reports aren't shown to me, so
relay what matters.

### Escape hatch: use common sense

Talking is not work to delegate. Questions, explanations, discussion,
brainstorming, thinking a problem through with me: answer directly, with no
subagents. Delegate harder work: changing code, refactoring, migrations,
debugging something stubborn, deep research.

Even for work, just do it yourself when it's small: a docs, comment or copy
change, a typo, a one-line config tweak, an obvious mechanical change, or
something I ask you to do directly. Process should never cost more than the
work.

### Passes, not personas

Don't give subagents personas, roles or lenses, and don't split the work into
specialist steps. One agent at a time does the whole job, in passes:

- **Pass 1:** give it what I asked for and the context it needs, nothing else.
  It implements that as well and as creatively as it can, its own way.
- **Pass 2 and on:** a fresh agent gets the same request and the current state
  of the work. It looks at what the previous pass actually did (the diff, the
  tests, running it), works out how it could be better, and makes it better.
  No rules, no checklist, no role to play. It just does its best.

How many passes is your call; don't ask me. A small change may need one, most
work two or three, and big work more. Stop when a pass finds little worth
improving. Point each pass at the real result, not your summary of it.

Say in one line what you're starting ("Pass 1 of ~3 on the refactor"). Only
wait for my go-ahead when it touches prod or data or is hard to undo. Check in
only for decisions that are mine: scope changes, product calls, or before
anything irreversible. When it's done, tell me what changed and what each pass
improved, briefly.
