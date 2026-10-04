use std::{
    io::Write,
    process::{Command, Stdio},
};

fn run(script: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_shr-desk"))
        .arg("simulate")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn manual_override_and_disconnect_survive_the_command_path() {
    let output =
        run("select 11\nlevel -3\npropose 11 -12\ndisconnect\nlevel 2\nreconnect\nstatus\nquit\n");
    assert!(output.contains("REFUSED: Disconnected"));
    assert!(
        output
            .lines()
            .last()
            .unwrap()
            .contains("-3.0 dB / pan +0 / mute false / hold true / target -12.0 dB")
    );
}

#[test]
fn invalid_gain_and_analysis_rotaries_do_not_edit_the_mix() {
    let output =
        run("level NaN\nlevel inf\nlevel 13\npage analysis\nmidi b0 10 7f\nstatus\nquit\n");
    assert_eq!(output.matches("REFUSED: dB must be finite").count(), 3);
    assert!(output.contains("REFUSED: rotaries unavailable on this draft"));
    assert!(
        output
            .lines()
            .last()
            .unwrap()
            .contains("-6.0 dB / pan +0 / mute false / hold false")
    );
}

#[test]
fn octave_injection_changes_focus_without_changing_the_mix() {
    let output = run("midi 90 3c 64\nstatus\nquit\n");
    assert!(
        output
            .lines()
            .last()
            .unwrap()
            .starts_with("CH 13 Input 13:")
    );
    assert!(output.contains("revision=0"));
}

#[test]
fn returning_to_mix_requires_fresh_pickup() {
    // Catch K01 near the initial -6 dB value, then leave and return to its page.
    let output = run("midi b0 10 69\npage analysis\npage mix\nmidi b0 10 00\nstatus\nquit\n");
    assert!(output.contains("PICKUP K01: approach"));
    assert!(!output.lines().last().unwrap().contains("-90.0 dB"));
}

#[test]
fn protected_pad_and_keyboard_paths_need_confirmation() {
    let keyboard = run("key H\nkey Enter\nkey R\nkey Esc\nstatus\nquit\n");
    let midi = run("midi 99 28 64\nmidi 99 24 64\nmidi 99 29 64\nmidi 99 25 64\nstatus\nquit\n");
    assert_eq!(keyboard.lines().last(), midi.lines().last());
    assert!(keyboard.lines().last().unwrap().contains("hold true"));
    assert!(keyboard.contains("PREVIEW"));
    assert!(midi.contains("PREVIEW"));
}
#[test]
fn shorthand_error_releases_enter_and_protected_drafts_are_not_replaced() {
    let output = run("key Enter\nkey H\nkey Enter\nstatus\nquit\n");
    assert!(output.contains("REFUSED: no draft"));
    assert!(output.lines().last().unwrap().contains("hold true"));
    let output = run("key R\nkey H\nkey Enter\nstatus\nquit\n");
    assert!(output.contains("REFUSED: another draft is open"));
    assert!(output.lines().last().unwrap().contains("hold false"));
}
