# Downloading files

Needs a server with API revision 13 or newer (`rex` snapshots from 2026-11-10;
preview.14 is revision 12). On an older server the dialog says "Rex on <host> is
too old to send files; update Rex there." and nothing is sent.

- **Starting one.** Choose **Download file…** in the pane menu (right-click, or
  `Ctrl Shift Space`) or in Commands (`Ctrl Shift P`), or bind `pane.download` to
  a key (it has no default; see [Configuring
  shortcuts](shortcuts.md#configuring-shortcuts)). The dialog is titled with the
  host of the pane it was opened from. Type a path on that machine: absolute,
  `~/…`, or relative to the pane's foreground program's directory (so it follows
  `cd`). The path is always on the machine running Rex, even when the pane shows
  an `ssh` session or a container: with `ssh remote` in the foreground, a relative
  name resolves in `ssh`'s own directory on the Rex host, not on `remote`, so type
  an absolute path there. The resolved path is the row's tooltip in the Downloads
  popover. A single-line selection in the pane fills the field. Enter starts;
  Escape closes. Only a person at this client starts a download: terminal output
  and `client.perform` cannot.
- **Where files go.** The **Save in** folder shows under the field: the XDG
  download folder (`xdg-user-dirs`, else `~/Downloads`), or `downloads.directory`
  in `workspace.json` (absolute or `~/…`), which **Change** in the dialog and
  Settings → Interface → Download folder write (the same check: absolute or
  `~/…`; empty goes back to the system's folder). The file keeps the server's
  file name after cleaning: control characters, direction overrides and invisible
  characters are removed, a leading dot becomes `_`, trailing dots and spaces go,
  and a name that ends up empty becomes `download`. A leading dash and non-ASCII
  names are kept as they are. The folder must already exist: it is never created,
  and a missing one fails with "<folder> doesn't exist." (use **Change**, or
  create it first; minimal desktops often have no `~/Downloads`).
- **Never overwritten.** If the name exists, the new file is `name (1).ext`, then
  `(2)`. The saved file is mode 0644 less the umask and never executable, whatever
  the source's mode. While running, the data goes to a hidden
  `.name.<uuid>.rex-part` in the same folder, renamed into place only when the
  transfer finished completely.
- **Never opened.** A finished transfer leaves one notice ("Saved build.log from
  devbox to ~/Downloads"); nothing is shown in a file manager or run. A finished
  row in the Downloads popover offers one action, **Show in folder** (the file
  manager's `ShowItems`, else `xdg-open` on the folder). Rex Rune does not open
  downloaded files itself; open them from your file manager.
- **Progress.** While bytes arrive, a chip in the toolbar shows `↓ build.log 42%`
  (or `↓ 3 files`); click it, or choose **Downloads** in Commands, for a row per
  transfer of this launch with its state and **Cancel**. Cancelling closes the
  stream and deletes the partial file. A failure is a sticky notice and a row that
  says why; nothing is retried or resumed.
- **Limits and confirmation.** A file of 1 GiB or more, or one that would leave
  under 256 MiB free, is not started until you answer a question in the dialog
  ("build.img is 3.2 GB. ~/Downloads has 41 GB free."), with **Cancel** focused. A
  file that cannot fit with 16 MiB left over is refused at once. After a yes to
  the low-space question the transfer stops if free space falls under 16 MiB;
  after a yes to a large file it stops under 256 MiB. A transfer ends if the file
  grows past the larger of twice Rex's size and that size plus 64 MiB (a file
  being appended to), or if no data arrives for 30 seconds. A file that changes
  while it is sent is saved as it arrived: one that grew by less than that is
  saved longer than Rex first reported, and one that shrank or whose reported size
  was wrong (`/proc` and `/sys` files) is saved shorter; the Saved notice adds the
  size when it differs from what Rex reported ("6 bytes; Rex reported 4096"). A
  log, database or archive that is being written to can therefore be saved as a
  torn copy and still say Saved, so download files that are not being written to.
  At most three downloads run at once per host; a fourth is refused. There is no
  other size limit.
- **Leftovers.** Quitting the app with the Close button or the window manager's
  close cancels every running download without asking, and a transfer is never
  resumed on the next launch, so a large file that is nearly done is lost. A
  partial file is deleted on cancel, on failure and when the app quits that way;
  each is recorded in `$XDG_STATE_HOME/rex-rune/downloads.json` (on macOS, in
  `~/Library/Application Support/rex-rune`). The app has no signal handler: a
  signal (SIGTERM, SIGINT, SIGHUP) or a crash ends it at once and leaves the
  partial file until the next launch, which deletes it. Only recorded files are
  touched; the folder is never swept.
- **Known limits.** Filesystems that support neither a no-replace rename nor hard
  links (sshfs, CIFS without Unix extensions) fail cleanly (the transfer ends with
  an error and the partial file is deleted). A download folder that is itself a
  symlink is refused, and the message names the real folder. A transfer ends if
  the connection to the host drops, and is never resumed or retried; Rex ends
  every stream when a control connection goes.

`python3 scripts/download-acceptance.py` checks this behaviour against the built
binary (see [Development](building.md#development)); HTTPS and Tailscale hosts,
the real file-manager and `xdg-open` calls and Wayland pointer input were not part
of that run.

Uploading files to the terminal's machine is a separate path; see [Uploading
files](terminal.md#uploading-files).
