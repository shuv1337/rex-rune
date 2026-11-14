# Accessibility

Rune's AT-SPI tree is enabled (Rune's default `accessibility` feature). Rune
builds it only while the desktop's accessibility bus has `org.a11y.Status
IsEnabled` set. It was inspected with an AT-SPI client script on a private bus;
**it has not been tried with Orca or any other screen reader, so what is spoken
is unknown.**

What a client sees:

- **Chrome.** Toolbar, sidebar, Settings, dialog and notice buttons have
  accessible names (the resize handles do not). Toolbar tabs, Settings sections,
  switches (toggle buttons with a pressed state), segmented controls (radio buttons
  with a checked state) and text fields (with caret, selection and edit events, and
  `EditableText`) have roles and states. Dialogs have role dialog, are named by
  their title, move focus to a button or tab inside, drop the window behind them
  from the tree while open, and close on Escape. The switcher and Commands lists
  report the highlighted row as the focused item as you move through it (the
  search field then no longer reports focus itself). Notices (toasts and the
  update notice) are status regions and raise AT-SPI announcements when they
  appear while a client is attached. Rex has no password fields.
- **Terminal panes.** A pane is a button named `Terminal <pane name>` holding one
  panel per visible row and one label per text segment. The labels carry the
  visible text and update as output arrives (about 0.1 to 0.2 s after a shell
  command's output was sent, in a test); nothing off screen, from another tab,
  session or host, or from scrollback is exposed. There is no text interface,
  caret, selection or terminal role, so a client reads node names rather than
  lines, words or characters, and each redraw replaces nodes. The pane is not
  focusable and keyboard focus is reported on the window, not the terminal.
- **Keyboard.** Ordinary keys go to the terminal by design. Without a pointer you
  can reach the rest through Commands (Ctrl Shift P): it lists Settings, Add host,
  tabs, panes, sessions and the other chrome actions. Inside Settings and dialogs,
  Tab, arrow keys and Escape work. No item of the pane menu took focus when it
  opened and Down was pressed, and the toolbar and sidebar buttons are not
  focusable. Clients can still press every named button, tab, switch and radio
  button with the AT-SPI `click` action, and focus Settings controls with
  `grab_focus`.
- **Cost.** With the bus enabled the tree is built and sent on every redraw, even
  when no screen reader is attached (about 215 nodes per update, 0.3 to 0.4 ms
  each, 95th percentile under 0.7 ms). While a full-screen program redrew 20 times
  a second, the app used about 0.71 CPU cores with the bus enabled and no client
  attached, about 0.78 while a script walked the whole tree once a second, and
  about 0.59 with the bus disabled (the pre-bump build: 0.66); three interleaved
  rounds on a busy machine, so treat the differences as rough.
  Resident memory rose by about 5 MB. A client walking the tree during a redraw
  sometimes hit stale nodes (3 to 5 failed reads in 30 walks). The release binary
  is about 13 MB larger than before the bump; about 5 MB of that is the
  accessibility feature (51.2 MB without it, 56.1 MB with it).

Known gaps: terminal text interface and terminal role; terminal pane focus;
keyboard access to the pane menu and the toolbar and sidebar buttons; the
sidebar's session rows (buttons whose names end in "(current)") have no selected
state; group labels for segmented controls and setting titles are separate nodes,
not associated with their controls; resize handles, the dialog scrim and the
window-sized press targets are unnamed nodes; a notice raises four announcement
events (its two nested status regions, text and Dismiss button), which a screen
reader might read more than once.
