# Verify input methods in a native window

Build the IME example and the CLI.

```sh
MACOSX_DEPLOYMENT_TARGET=15.4 cargo build -p octara-examples-desktop --bin ime -p octara-cli --bin octara
python3 -B examples/ime_window.py
```

Open the reported `bundle` and click **Single line**. Enter text with an emoji, switch to an enabled Pinyin input method, and type Pinyin. Use the candidate window to commit the text, then switch to the multiline editor and check which field owns the marked text. Confirm focus before inspecting each single-line, multiline, or secure field. Secure field receipts expose only security flags and session identifiers.

At each stage, save the focused editor’s session, text revision, selection, and composition range to a separate JSON file. Set `socket` to the path printed by the script.

```sh
target/debug/octara inspect --connect "$socket" --json
python3 -B examples/ime_observe.py --socket "$socket" --output composing.json
```

Selection offsets in `snapshot.editor` count UTF-8 bytes. Click **Remount single-line field**, focus the field again, and compare the old and new sessions and targets. Use the app snapshot to verify framework text. To capture the system candidate window separately, run `swift examples/ime_candidates.swift --pid PID --output NEW_DIRECTORY` with the target process ID. [macOS text client](../crates/octara-host-native/src/os/macos/editor.rs)

To test inline file renaming, run `python3 -B examples/ime_window.py --scene rename`. Select **Rename B** and replace the filename using the system input method. While composing, send `rename.reorder` or `rename.refresh` and check `snapshot.editor` to see whether it retains the same session. Use `rename.external` to simulate a concurrent write, then check `rename-status` after committing to verify conflict handling.

On iOS, install the same example with `--scene ime` and type with the simulator’s system keyboard. [iOS Simulator instructions](README.md#ios-simulator) · [iOS text client](../crates/octara-host-native/src/os/ios/editor.rs)
