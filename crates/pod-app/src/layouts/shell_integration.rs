//! OSC 133 prompt marks for the shells **Pod itself starts** (the 셸 preset of
//! the new-terminal dialog, a task's "셸 열기", or a custom command that is just
//! a bare shell). The 블록 터미널 layout cuts command blocks at these marks
//! (docs/10 §D5.8). Agents, other programs and shells started outside Pod are
//! never touched; nothing is written to the user's profile.
//!
//! - **PowerShell** (5.1 and 7): `-NoExit -EncodedCommand` wraps the prompt
//!   function the user's profile defined: `D;<exit>` + `A` before it, `B` after
//!   it, and — when PSReadLine is loaded — `C` when Enter accepts a line.
//! - **cmd**: `/K prompt …` with `D`, `A` and `B` (cmd has no hook before a
//!   command runs; the block's output starts on the line after the input).

/// The PowerShell snippet (readable form; sent UTF-16LE base64-encoded).
pub const POWERSHELL: &str = r#"$global:__PodFirst = $true
$global:__PodPrompt = $function:prompt
function global:prompt {
  $ok = $?; $code = $global:LASTEXITCODE
  $e = [char]27; $b = [char]7; $s = ''
  if (-not $global:__PodFirst) {
    if ($ok) { $c = 0 } elseif ($code) { $c = $code } else { $c = 1 }
    $s += "$e]133;D;$c$b"
  }
  $global:__PodFirst = $false
  $p = if ($global:__PodPrompt) { & $global:__PodPrompt } else { "PS $($executionContext.SessionState.Path.CurrentLocation)> " }
  $global:LASTEXITCODE = $code
  "$s$e]133;A$b$p$e]133;B$b"
}
if (Get-Module PSReadLine) {
  Set-PSReadLineKeyHandler -Chord Enter -ScriptBlock {
    [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    [Console]::Write("$([char]27)]133;C$([char]7)")
  }
}"#;

/// The cmd prompt (`$e` = ESC, `$e\` = ST): no spaces, so it needs no quoting.
pub const CMD_PROMPT: &str = r"$e]133;D$e\$e]133;A$e\$P$G$e]133;B$e\";

fn exe(arg: &str) -> String {
    let name = arg
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(arg)
        .to_ascii_lowercase();
    name.strip_suffix(".exe").map(str::to_owned).unwrap_or(name)
}

/// PowerShell flags that keep it an interactive shell.
const PS_SAFE: [&str; 5] = ["-nologo", "-noprofile", "-nol", "-nop", "-interactive"];

/// `argv` with prompt marks when it is a bare interactive PowerShell or cmd;
/// anything else (agents, scripts, `-Command`, `/C` …) comes back unchanged.
pub fn integrate(argv: Vec<String>) -> Vec<String> {
    let Some(first) = argv.first() else {
        return argv;
    };
    match exe(first).as_str() {
        "powershell" | "pwsh"
            if argv[1..]
                .iter()
                .all(|a| PS_SAFE.contains(&a.to_ascii_lowercase().as_str())) =>
        {
            let mut out = argv;
            out.extend([
                "-NoExit".to_string(),
                "-EncodedCommand".to_string(),
                encode_command(POWERSHELL),
            ]);
            out
        }
        "cmd" if argv.len() == 1 => vec![
            first.clone(),
            "/K".into(),
            "prompt".into(),
            CMD_PROMPT.into(),
        ],
        _ => argv,
    }
}

/// `-EncodedCommand` form: base64 of the UTF-16LE text.
pub fn encode_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_bare_shells_get_marks() {
        let ps = integrate(v(&["powershell.exe", "-NoLogo"]));
        assert_eq!(
            ps[..4],
            v(&["powershell.exe", "-NoLogo", "-NoExit", "-EncodedCommand"])
        );
        assert_eq!(ps[4], encode_command(POWERSHELL));
        assert_eq!(
            integrate(v(&["C:\\Program Files\\PowerShell\\7\\pwsh.exe"])).len(),
            4
        );
        assert_eq!(
            integrate(v(&["cmd.exe"])),
            v(&["cmd.exe", "/K", "prompt", CMD_PROMPT])
        );
        for untouched in [
            v(&["claude"]),
            v(&["codex", "--model", "x"]),
            v(&["powershell.exe", "-File", "a.ps1"]),
            v(&["pwsh", "-Command", "ls"]),
            v(&["cmd", "/C", "dir"]),
            v(&[]),
        ] {
            assert_eq!(integrate(untouched.clone()), untouched);
        }
        assert!(!CMD_PROMPT.contains(' '), "cmd prompt needs no quoting");
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        // `dir` in UTF-16LE, as `powershell -EncodedCommand` expects.
        assert_eq!(encode_command("dir"), "ZABpAHIA");
        assert!(
            POWERSHELL.contains("133;A")
                && POWERSHELL.contains("133;B")
                && POWERSHELL.contains("133;C")
                && POWERSHELL.contains("133;D")
        );
    }
}
