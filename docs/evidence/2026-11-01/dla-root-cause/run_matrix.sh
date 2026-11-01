#!/bin/sh
# Owned-process PEBS DLA matrix. Run from the directory holding ./pebsdiag.
# Every case samples only the pebsdiag process's own thread, pinned to $CPU.
CPU=${CPU:-30}
P=2000003
run() { echo "### $*"; "$@" 2>&1; echo "exit=$?"; }
echo "## host"; uname -r; grep -m1 microcode /proc/cpuinfo; grep -m1 'model name' /proc/cpuinfo
cat /sys/bus/event_source/devices/cpu/caps/max_precise /proc/sys/kernel/perf_event_max_sample_rate
echo "## iomem entries containing 0xfeb80000"
python3 -c "
for l in open('/proc/iomem'):
    r,_,n=l.strip().partition(' : '); a,b=(int(x,16) for x in r.split('-'))
    if a<=0xfeb80000<=b: print(l.rstrip())
print('end')"
echo "## L1-resident sequential loads (16 KiB)"
for ev in 81d0 01d1 01c2; do for pr in u uk; do run ./pebsdiag $CPU seq $ev 0 $P 2 $pr 1; done; done
run ./pebsdiag $CPU seq 81d0 0 $P 1 u 1
run ./pebsdiag $CPU seq 81d0 0 $P 3 u 1
run ./pebsdiag $CPU seq 81d0 0 200003 2 u 1
run ./pebsdiag 12 seq 81d0 0 $P 2 u 1
run ./pebsdiag $CPU seq 1cd 3 200003 2 u 1
WAKE0=1 run ./pebsdiag $CPU seq 81d0 0 $P 2 u 1
WAKE0=1 run ./pebsdiag $CPU seq 01d1 0 $P 2 u 1
run ./pebsdiag $CPU bss 81d0 0 $P 2 u 1
run ./pebsdiag $CPU bss 01d1 0 $P 2 u 1
run ./pebsdiag $CPU stack 81d0 0 $P 2 u 1
run ./pebsdiag $CPU syscall 81d0 0 $P 2 u 1
echo "## subsets of ALL_LOADS"
run ./pebsdiag $CPU split 41d0 0 200003 2 u 1
run ./pebsdiag $CPU split 41d0 0 200003 2 uk 1
run ./pebsdiag $CPU lock 21d0 0 200003 2 u 1
run ./pebsdiag $CPU lock 21d0 0 200003 2 uk 1
run ./pebsdiag $CPU chase 11d0 0 20011 2 u 1.5
run ./pebsdiag $CPU chase 11d0 0 20011 2 uk 1.5
echo "## data-source events by residency"
export CHASE_BYTES=131072
for ev in 81d0 67d1 02d1; do run ./pebsdiag $CPU chase $ev 0 20011 2 u 1; done
run ./pebsdiag $CPU chase 02d1 0 20011 2 uk 1
export CHASE_BYTES=4194304
for ev in 81d0 67d1 04d1; do run ./pebsdiag $CPU chase $ev 0 20011 2 u 1; done
run ./pebsdiag $CPU chase 04d1 0 20011 2 uk 1
run ./pebsdiag $CPU chase 08d1 0 20011 2 u 1
run ./pebsdiag $CPU chase 08d1 0 20011 2 uk 1
export CHASE_BYTES=268435456
for ev in 81d0 67d1 20d1; do run ./pebsdiag $CPU chase $ev 0 20011 2 u 1.5; done
run ./pebsdiag $CPU chase 20d1 0 20011 2 uk 1.5
echo "## mixed L1 + DRAM: union umask count vs ALL_LOADS"
export CHASE_BYTES=67108864
for ev in 81d0 67d1 27d1 01d1 20d1 40d1; do run ./pebsdiag $CPU mixed $ev 0 100003 2 u 1.5; done
echo "## stores"
run ./pebsdiag $CPU store 82d0 0 $P 2 u 1
run ./pebsdiag $CPU store 82d0 0 $P 2 uk 1
run ./pebsdiag $CPU store 67d1 0 $P 2 u 1
run ./pebsdiag $CPU seq 67d1 0 $P 2 u 1
echo "## done"
