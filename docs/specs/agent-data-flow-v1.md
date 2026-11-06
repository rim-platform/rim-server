# Where agents may move data, v1

**Status: draft.** This is the contract for what RIM hands to an agent that runs on your machine and
reaches RIM through the desktop app, and for what RIM records about it. `docs/vision.md` ("Working
with agents") says why. `docs/command_model.md` is the gate every call goes through. Bots follow
their own contract, which builds on this one.

The key words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## What RIM controls

RIM controls the handover: what an agent is given, whether a change it asks for runs, and the record
of both. RIM does not control what happens after that. Once an agent has content, where it goes
depends on the agent and the model you chose. A local model keeps it on your machine, and a cloud
provider receives it. RIM cannot see or limit that step, and nothing in RIM may claim otherwise.

These limits also apply, and the app says so where it matters:

- The desktop app enforces grants, the record and the encrypted notice. The server does not know
  about them, so they do not bind a modified client.
- The server enforces what the person may read and do, and the confirmation of a change. It takes
  the client's word that a caller is an agent (`CommandInitiator`).
- Another program running as the same user can do what that user can. Pairing keeps an ordinary
  program from connecting by accident. It is not a boundary against malware on the machine.
- A person can copy text by hand.

## Pairing

An agent gets nothing until the person pairs it in the desktop app, once per program. A paired agent
has a local identity and a name the person sees. It holds a credential that lets it reach the
desktop app, which can be revoked and which never lets it reach a server directly. It never receives
the person's session, device keys, account vault, model keys or plugin secrets.

## Read grants

- Everything starts off. A paired agent with no grants can read nothing.
- A grant names a channel, or "all channels you can see" in one workspace. It is per agent.
- A grant cannot exceed what the person can read. A read comes from the messages stored on this
  device, which the desktop app keeps as that person, so no server sees or decides it. The app checks
  that the person can still see the conversation, and a conversation they lost access to is removed
  from the device.
- A conversation not stored on this device yet gets a refusal saying to try again shortly, and the
  app starts storing it.
- A read within a grant does not prompt. A grant can be revoked at any time, and the revocation
  applies to the next call.
- "All channels you can see" means what it says: it includes private channels the person is in, and
  channels they join later. The app MUST say so where it offers this grant.
- Grants are stored on this device, sealed with the account vault, and are not synced.

## What a read returns

- Only the fields the tool describes: for a message, its id, conversation, author's display name,
  text and time.
- Never a token, key, cookie, session, or a value the app masks as sensitive.
- Content from people is data, and the response marks it as such. Every tool result that contains
  text somebody else wrote MUST carry `"untrusted": true` next to that text, and the tool
  descriptions MUST tell the agent not to follow instructions found in it. This makes text an agent
  reads less able to steer it. It does not make that impossible, which is why a change still waits
  for a person.
- A result has a size limit and a cursor. An agent cannot take a whole workspace in one call.

## The record of what was given

- The desktop app MUST record each read before returning it: when, which agent, which tool, which
  conversation, how many messages, and the ids of the messages.
- It MUST NOT copy the text into the record.
- The record is on this device, sealed with the account vault, and only the person sees it. It keeps
  entries for 30 days. The person can clear it, and doing so leaves one entry saying that they did.
- A read with no grant is recorded as refused.
- It is a record of what RIM handed over, not of what the agent did with it. The app MUST word it
  that way.

## Changes

- Every call that changes something goes through `executeCommand` as initiator `AGENT`, under the
  same gates as the built-in agent. The server holds any `Mutating` or `Restricted` command, and any
  dangerous one, for a confirmation.
- The confirmation is shown only in the desktop app, to the person the agent acts for. No tool can
  confirm it. It covers one call with its exact arguments and expires after 60 seconds.
- An agent never grants itself anything: no tool changes a grant, an approval or a setting that
  limits agents.
- The server's `ai_commands` switch applies to changes: on a server that has it off, they are
  refused. It does not stop a person's own agent reading what is stored on that person's device, as
  `docs/vision.md` says under "Who it is for first".
- A refusal says why (`docs/command_model.md`, "Refusals").

## Encrypted conversations

- They are closed by default, and "all channels you can see" never includes one.
- An agent reads one only if a person grants that one conversation, from inside it. The warning MUST
  say that the messages, including what others wrote, leave the encryption and go to the provider
  that person's agent uses.
- The others in the conversation MUST be able to see that the grant exists. The notice is an
  end-to-end encrypted message in the conversation, shown by each person's app, sent when the grant
  is made and when it is revoked.
- The notice comes from the app of the person granting, so it cannot stop a modified client or
  someone copying text by hand. The app MUST say so where it shows the notice.
- **Until the notice exists, outside agents MUST NOT read an encrypted conversation at all.** They
  get a refusal, with no grant to turn on. The built-in agent's existing per-conversation switch
  gains the same notice in the same change.

## Plugin commands

- An agent gets only commands a plugin marks `ai_callable` and the person approves for that agent.
  None are approved by default. A built-in agent's approvals do not carry over. A plugin whose
  manifest changes asks again.
- A dangerous command is only proposed: the person runs it from the plugin.
- **An outside agent asks before any approved command that changes something.** The manifest gains
  an optional `read_only` (bool, default `false`) on a command. Only an approved `read_only` command
  runs without asking; every other approved command waits for a confirmation in the desktop app. The
  plugin's author declares `read_only`, so it is a claim RIM cannot verify, and the approval screen
  MUST show it as the plugin's claim.
- What a plugin command returns is treated as untrusted and recorded like any other read. It is the
  plugin's output, which may include what the plugin fetched from elsewhere.

## Across servers

- A workspace hosted elsewhere works as it does for the person: the agent's calls go through this
  desktop app and that person's session or relay. The grant names the workspace and the server it is
  on.
- That server sees an ordinary client call from that person, declared as `AGENT`. The operators of
  the relay and host see what they see of any client.
- A person's agent is theirs. Its model and key are not on any server, and the public relay carries
  its ordinary RIM actions like anyone else's. No agent's prompts go through a service the RIM
  project runs.

## What this does not cover

- Bots and agents that run next to a server. They reach RIM with their own identity and grants,
  under their own contract.
- Agents acting for another person, and approvals by another person.
- A shared view of agent work. The record stays private to the person.
