// Written by `island-bridge hooks install`; `island-bridge hooks remove` deletes it.
// Asks the island about an OpenCode permission. If the hook fails or prints nothing,
// output.status is left as OpenCode set it.
import { spawn } from "node:child_process";

const EXE = __EXE__;

export const IslandApproval = async () => ({
  "permission.ask": async (input, output) => {
    const reply = await run(JSON.stringify(input));
    const status = reply && reply.status;
    if (status === "allow" || status === "deny" || status === "ask") output.status = status;
  },
});

function run(stdin) {
  return new Promise((resolve) => {
    let out = "";
    let child;
    try {
      child = spawn(EXE, ["hook", "opencode"], { env: process.env, stdio: ["pipe", "pipe", "ignore"] });
    } catch {
      return resolve(null);
    }
    child.stdout.on("data", (chunk) => (out += chunk));
    child.on("error", () => resolve(null));
    child.on("close", () => {
      try {
        resolve(JSON.parse(out));
      } catch {
        resolve(null);
      }
    });
    child.stdin.end(stdin);
  });
}
