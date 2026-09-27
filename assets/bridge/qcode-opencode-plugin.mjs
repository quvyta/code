// The QCode bridge as an opencode plugin, for opencode's shared server.
//
// Every opencode tab of a profile is a client of one server in the profile's container
// (`qcode-opencode.mjs`), so the MCP servers opencode starts are started once, by the server, and
// cannot tell one tab from another: the token in their environment is the server's. A plugin tool
// is told the conversation it is called from, and each tab shows one conversation of its own, so
// the question to QCode carries that conversation beside the server's token and QCode finds the
// tab by the two together.
//
// A conversation an agent of the tab started for a helper (a subagent) is a child of the tab's
// own; the chain of parents is followed up to the one the tab shows.
//
// The tools are the MCP server's, from the same file, under the names opencode gives an MCP
// server's tools (`qcode_list_tabs`), so an agent reads the same tools either way. Their
// arguments are written as JSON Schema: opencode takes a plugin's arguments as Zod schemas or,
// failing that, as JSON Schema (read in its 1.18.32 build), and nothing but Node is promised in
// the image, so there is no Zod to import.

import { TOOLS, call } from "./qcode-bridge.mjs";

// Deeper than any chain of helpers an agent really starts, finite so a loop in what the server
// answers cannot hold a tool call forever.
const MOST_PARENTS = 16;

export const QCodeBridge = async ({ client }) => {
  // The conversation a tab shows, reached from the one a tool was called in.
  async function shown(session) {
    let id = session;
    for (let depth = 0; depth < MOST_PARENTS; depth += 1) {
      let parent;
      try {
        const found = await client.session.get({ path: { id } });
        parent = found?.data?.parentID;
      } catch {
        return id;
      }
      if (typeof parent !== "string" || parent === "") return id;
      id = parent;
    }
    return id;
  }

  const tool = {};
  for (const definition of TOOLS) {
    tool[`qcode_${definition.name}`] = {
      description: definition.description,
      args: definition.inputSchema.properties ?? {},
      async execute(args, context) {
        const session = await shown(context.sessionID);
        const result = await call({ name: definition.name, arguments: args ?? {} }, { session });
        return result?.content?.[0]?.text ?? "";
      },
    };
  }
  return { tool };
};
