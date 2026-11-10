use demo_app::geometry::{perimeter, Point};
use demo_app::{greet, word_count, Task};

fn main() {
    let message = greet("Pod");
    println!("{message} ({} words)", word_count(&message));

    let square = [
        Point { x: 0.0, y: 0.0 },
        Point { x: 1.0, y: 0.0 },
        Point { x: 1.0, y: 1.0 },
        Point { x: 0.0, y: 1.0 },
    ];
    println!("perimeter = {}", perimeter(&square));

    let mut task = Task::new("에디터 만들기");
    task.finish();
    println!("{task:?}");
}
