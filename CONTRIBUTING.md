# Contributing to Radio Collector

Thank you for your interest in contributing to **Radio Collector**.

---

## Before Opening an Issue

- Search existing issues to verify the problem has not already been reported or solved.
- Provide enough technical details to isolate the root cause.
- Include logs from `journalctl -u radio-collector.service` or terminal output when applicable.

---

## Reporting Bugs

When filing a bug report, please provide:

- **Environment**: Operating System distribution and version (e.g., Debian 13 Trixie x86_64 / ARM64).
- **Rust Toolchain**: `rustc --version` and `cargo --version`.
- **yt-dlp Version**: `yt-dlp --version`.
- **ffmpeg & Node.js**: `ffmpeg -version` and `node --version`.
- **Radio Stream URL**: The ICY stream URL being monitored.
- **Steps to Reproduce**: How to consistently reproduce the issue.
- **Expected Behavior**: What you expected to occur.
- **Actual Behavior**: What actually happened (including error messages, stderr, or screenshot).

---

## Feature Requests

- Clearly explain the real-world operational problem or use case first.
- Propose the technical solution, architectural design, or interface enhancement.

---

## Pull Requests

- **Small PRs preferred**: Keep changes focused and atomic.
- **Engineering Decisions**: Clearly justify any new external crate dependency or protocol change.
- **Code Style & Quality**: Ensure code complies with `cargo fmt --check` and passes strict linting (`cargo clippy -- -D warnings`).
- **Zero Warnings Policy**: New submissions must produce zero compiler warnings and zero dead code.

---

## AI-Assisted Contributions

AI-assisted contributions are welcome.

However:

- Contributors remain strictly responsible for any submitted code;
- Generated code must be thoroughly reviewed and tested;
- Contributors should understand the underlying engineering logic and trade-offs of what they submit;
- AI-generated code must never be merged without human engineering validation.
