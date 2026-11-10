// Demo for screenshots (crates/pod-app/tools/mods-demo.ps1): announce when a
// mod-detected terminal starts waiting on a question.
pod.on("status.changed", (ev) => {
  if (ev.source === "mod" && ev.state === "blocked") {
    pod.notify("에이전트가 답을 기다립니다", `${ev.terminal}: ${ev.badge} (확장 감지)`, "warning");
  }
});
