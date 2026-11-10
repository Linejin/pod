// Fixture TypeScript file.
export interface Task {
  id: number;
  title: string;
  done: boolean;
}

export function summarize(tasks: Task[]): string {
  const done = tasks.filter((t) => t.done).length;
  return `${done}/${tasks.length} done`;
}

const tasks: Task[] = [
  { id: 1, title: "tree", done: true },
  { id: 2, title: "quick open", done: false },
];

console.log(summarize(tasks));
