# Results of the image under QEMU (emulation: timings meaningless)

`RESULTS.TXT` and `STATE.TXT`: the full run of `../qemu.sh` with
`../qemu-config.txt` and `SMP=30,sockets=1,cores=15,threads=2` (14 workers,
14 second threads, one of them the watchdog), four boots (three restarts of
`warm-reset`, one per method with `quick=1`). `RESULTS-SHOOT-14CORES.TXT`:
`shoot` alone with `SMP=28,sockets=1,cores=14,threads=2`, the desktop's count
of physical cores, where the shootdown to 13 others includes the bootstrap
core. They show which paths run, and are the input of the tables tool's test.

Note (work paused, 16 November 2026): these files come from the image as
built before the last three small commits of the branch (a heartbeat line
now also flushes the results, the start-up key help was reworded so that no
screen line starts with `R `, and the bootstrap core's restart re-arms a
TSC-deadline timer). The run with the final image was stopped during `init`
when the work was paused; it is to be redone (see the pull request).
