# Run sheet: the measurement stick on the reference desktop

One page for the owner at the machine. Background in [README.md](README.md).

## 1. Make the stick (macOS, before going to the machine)

Any USB stick of 1 GB or more; **everything on it is erased**.

    nix develop -c sh -c 'cd trials/desktop/image && cargo build --release'
    diskutil list                       # find the stick, e.g. /dev/disk4: check its size!
    diskutil eraseDisk FAT32 DODOS MBR /dev/disk4   # one FAT32 partition
    mkdir -p /Volumes/DODOS/EFI/BOOT
    cp trials/desktop/image/target/x86_64-unknown-uefi/release/dodos-trial-desktop.efi \
       /Volumes/DODOS/EFI/BOOT/BOOTX64.EFI
    diskutil eject /Volumes/DODOS

No configuration file is needed. `\EFI\BOOT\BOOTX64.EFI` is the file a UEFI
firmware boots from removable media (`EFI_REMOVABLE_MEDIA_FILE_NAME_X64` in
the EDK2 headers).

## 2. Firmware settings (enter setup, usually Del or F2)

Names differ between boards; look for these and note what you changed:

| Setting | Set to | Why |
|---|---|---|
| Secure Boot | Disabled | The image is not signed |
| CSM / Legacy boot | Disabled (UEFI only) | The stick must boot as UEFI |
| Fast Boot | Disabled | Fast boot may skip USB devices |
| Boot order | The USB stick first | The run restarts the machine six times by itself and must come back to the stick |
| Intel Virtualization Technology (VT-x) | Enabled | For the `ept` step and the INIT-in-VMX case (VT-d does not matter) |
| C-states (and "package C-state limit") | Enabled / unlimited | To measure the exit time of each sleep state |
| Hyper-Threading | Enabled | The image's watchdog runs on a second hardware thread |
| E-cores | All enabled | Both kinds are measured |

Leave everything else (turbo, memory profile) as you would use the machine,
and write it down.

## 3. Run

Plug the stick, power on (or pick the stick once in the boot menu, F8/F11/F12,
if it is not first: but the restarts need it first). What a normal run
looks like:

1. A banner and 5 s for keys: **S** skips the risky steps, **R** forgets the
   progress of an earlier run, **Q** stops; no key: it goes on.
2. Lines `I step <name> begins` / `ends`, and many `R ...` result lines.
   The long one is `trial` (about 17 minutes, a table every few minutes).
3. Before each risky step (`ept`, `warm-reset`, `init`, `bsp`): "Any key
   within 3 s skips it". Do nothing.
4. `warm-reset` restarts the machine **six times by itself**; each time the
   firmware starts again and the image continues where it was.
5. At the end: "Done. Copy \DODOS from the stick. Any key powers the machine
   off."

A full run takes **about 40 minutes** including the restarts.

## 4. If it hangs

- The image has its own watchdog (it resets the machine when a step runs
  past its limit), so a frozen screen usually fixes itself. Wait **5
  minutes** (30 minutes if the last line is from `trial` and no new table
  came), then press reset or hold the power button.
- After any reset the run **resumes by itself** at the next step; the step
  that hung is reported as `F <step> hung` in the results. You do not need
  to do anything else.
- If it hangs again at the same place, restart and press **S** in the first
  5 s (skips every risky step), or note the step and tell us.
- Note on paper each manual reset: time, last line on screen (a phone photo
  of the screen is best).

## 5. What to send back

- The whole `\DODOS` folder from the stick (`RESULTS.TXT`, `STATE.TXT`,
  `TRIAL-*.CSV`, `CONFIG.TXT` if present).
- The board's make and model and its firmware version (shown in setup).
- The settings you changed, and your notes and photos of any hang.

Do not edit the files; we turn them into tables with
`cargo run -p dodos-trial-desktop-tables -- RESULTS.TXT`.
