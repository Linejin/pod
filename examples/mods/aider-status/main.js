// aider-status: a status detector for the Aider CLI (docs/16 §7.1).
//
// Aider sends no hooks and no OSC 7501 reports, so Pod would only have its
// generic heuristics. This mod reads the visible screen of terminals started
// as `aider` and reports a state with source "mod" (below hook and osc).
//
// Reading the screen bottom-up:
//   "... (Y)es/(N)o ..."            → blocked:question (Aider asks before adding files, running commands)
//   a bare prompt ("> ", "ask> ")   → done if an edit/commit just happened, else idle
//   "Waiting for …" / spinner       → working

const PROMPT = /^[a-z-]*>\s*$/i;
const QUESTION = /\(Y\)es\/\(N\)o/;
const SPINNER = /[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]/;
const WORKING = /^(Waiting for |Updating repo map|Scanning repo|Thinking)/;
const FINISHED = /^(Applied edit to |Commit [0-9a-f]{7,} )/;

pod.detector({
  match: ["aider"],
  onScreen(ctx) {
    const lines = ctx.lines.map((l) => l.replace(/\s+$/, "")).filter((l) => l.length > 0);
    const last = lines.length ? lines[lines.length - 1] : "";
    if (QUESTION.test(last)) {
      return { state: "blocked", kind: "question", msg: last.slice(0, 120) };
    }
    if (PROMPT.test(last)) {
      // Look a few lines above the prompt for the end of an edit.
      const recent = lines.slice(-6, -1);
      return recent.some((l) => FINISHED.test(l)) ? { state: "done" } : { state: "idle" };
    }
    if (lines.slice(-3).some((l) => WORKING.test(l) || SPINNER.test(l))) {
      return { state: "working" };
    }
    return null; // not sure: leave the state as it is
  },
});

console.log("aider-status ready");
