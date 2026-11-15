// Written by `island-bridge hooks install`; `island-bridge hooks remove` deletes it.
// Asks the island about an OpenCode permission. If the hook fails or prints nothing,
// output.status is left as OpenCode set it.
import { spawn } from "node:child_process";

const EXE = __EXE__;
// The hook waits up to 45 s for the user; give up a little after that.
const TIMEOUT_MS = 60_000;

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
    const timer = setTimeout(() => {
      if (child) child.kill();
      resolve(null);
    }, TIMEOUT_MS);
    const done = (value) => {
      clearTimeout(timer);
      resolve(value);
    };
    try {
      child = spawn(EXE, ["hook", "opencode"], { env: process.env, stdio: ["pipe", "pipe", "ignore"] });
    } catch {
      return done(null);
    }
    // an early exit must not turn into an unhandled EPIPE in the host
    child.stdin.on("error", () => {});
    child.stdout.on("data", (chunk) => (out += chunk));
    child.on("error", () => done(null));
    child.on("close", () => {
      try {
        done(JSON.parse(out));
      } catch {
        done(null);
      }
    });
    child.stdin.end(stdin);
  });
}
