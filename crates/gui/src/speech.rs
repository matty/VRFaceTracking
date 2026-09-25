//! Reads recording prompts aloud, so someone wearing the headset doesn't have
//! to find the screen. Windows' own speech runs in a hidden PowerShell that
//! takes one line to say at a time; it starts on the first prompt and ends
//! with the app.
use std::io::Write as _;
use std::process::{Child, ChildStdin, Command, Stdio};

/// Says each line from stdin, cutting off whatever it was still saying.
/// Windows PowerShell 5.1 ships with every Windows and has System.Speech.
const SCRIPT: &str = "Add-Type -AssemblyName System.Speech; \
    $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
    while ($null -ne ($l = [Console]::In.ReadLine())) { \
        $s.SpeakAsyncCancelAll(); if ($l) { [void]$s.SpeakAsync($l) } }";

#[derive(Default)]
pub struct Speaker {
    voice: Option<(Child, ChildStdin)>,
    /// Starting the voice failed once; don't retry for every prompt.
    unavailable: bool,
}

impl Speaker {
    pub fn say(&mut self, text: &str) {
        self.send(&one_line(text));
    }

    /// Stops speaking.
    pub fn hush(&mut self) {
        if self.voice.is_some() {
            self.send("");
        }
    }

    fn send(&mut self, line: &str) {
        if self.voice.is_none() && !self.unavailable {
            match start() {
                Ok(voice) => self.voice = Some(voice),
                Err(error) => {
                    log::warn!("Can't read prompts aloud: {error}");
                    self.unavailable = true;
                }
            }
        }
        let Some((_, input)) = &mut self.voice else {
            return;
        };
        if writeln!(input, "{line}")
            .and_then(|_| input.flush())
            .is_err()
        {
            // The voice has gone; start a new one next time.
            self.stop();
        }
    }

    fn stop(&mut self) {
        if let Some((mut child, input)) = self.voice.take() {
            drop(input);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn start() -> std::io::Result<(Child, ChildStdin)> {
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = crate::processes::hidden(&mut command).spawn()?;
    let input = child.stdin.take().expect("stdin is piped");
    Ok((child, input))
}

/// The prompt as one line of plain ASCII, which the console reads the same
/// under any code page.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{2013}' | '\u{2014}' => ',',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => ' ',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_become_one_plain_line() {
        assert_eq!(
            one_line("Tongue out \u{2014} halfway\nnow\u{2019}s"),
            "Tongue out , halfway now's"
        );
    }
}
