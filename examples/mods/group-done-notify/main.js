// group-done-notify: when every member task of a comparison group has
// finished its turn, post one notification (docs/16 §7).
//
// "Finished" = the member task's aggregate state is done, idle, error or
// exited, and at least one member is done. A group is announced once per
// round: when a member starts working again, the next finish counts anew.

const FINISHED = new Set(["done", "idle", "error", "exited"]);
const announced = new Set();

function check(group) {
  const states = group.members.map((m) => m.state || "unknown");
  if (states.length === 0) return;
  if (states.some((s) => s === "working" || s === "blocked")) {
    announced.delete(group.id);
    return;
  }
  const allFinished = states.every((s) => FINISHED.has(s));
  if (!allFinished || !states.includes("done") || announced.has(group.id)) return;
  announced.add(group.id);
  const failed = states.filter((s) => s === "error").length;
  pod.notify(
    `그룹 ${group.name}: 모든 에이전트 완료`,
    failed > 0 ? `${states.length}개 중 ${failed}개 오류. 결과를 비교해 보세요.` : `${states.length}개 작업이 끝났습니다. 결과를 비교해 보세요.`,
    failed > 0 ? "warning" : "success",
  );
}

pod.on("status.changed", (ev) => {
  if (!ev.task) return;
  for (const g of pod.groups()) {
    if (g.members.some((m) => m.task === ev.task)) check(g);
  }
});

pod.command({ id: "summary", title: "그룹 상태 요약", detail: "비교 그룹마다 끝난 작업 수" }, () => {
  const groups = pod.groups();
  if (groups.length === 0) {
    pod.notify("그룹 상태", "비교 그룹이 없습니다.");
    return;
  }
  const lines = groups.map((g) => {
    const done = g.members.filter((m) => FINISHED.has(m.state || "")).length;
    return `${g.name}: ${done}/${g.members.length} 완료`;
  });
  pod.notify("그룹 상태", lines.join("\n"));
});
