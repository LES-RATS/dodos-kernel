# Results of the image under QEMU (emulation: timings meaningless)

`RESULTS.TXT` and `STATE.TXT`: the full run of `../qemu.sh` with
`../qemu-config.txt` and `SMP=30,sockets=1,cores=15,threads=2` (14 workers,
14 second threads, one of them the watchdog), four boots (three restarts of
`warm-reset`, one per method with `quick=1`). `RESULTS-SHOOT-14CORES.TXT`:
`shoot` alone with `SMP=28,sockets=1,cores=14,threads=2`, the desktop's count
of physical cores, where the shootdown to 13 others includes the bootstrap
core. They show which paths run, and are the input of the tables tool's test.
