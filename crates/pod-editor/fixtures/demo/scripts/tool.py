"""Small helper script (fixture)."""

from dataclasses import dataclass


@dataclass
class Report:
    name: str
    count: int = 0

    def bump(self, by: int = 1) -> "Report":
        self.count += by
        return self


def main() -> None:
    report = Report("files").bump(3)
    print(f"{report.name}: {report.count}")


if __name__ == "__main__":
    main()
