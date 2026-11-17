#!/usr/bin/env bash
# Only for a disposable Ubuntu CI VM. Never changes global LSM/sysctl policy.
set -euo pipefail
: "${RUNNER_TEMP:?Run only in an owned CI VM}"
evidence="$RUNNER_TEMP/attune-workload-prerequisites"
mkdir -p "$evidence"
record() {
  uname -a
  id
  dpkg-query -W bubblewrap apparmor apparmor-profiles 2>/dev/null || true
  for path in /sys/module/apparmor/parameters/enabled /proc/sys/kernel/unprivileged_userns_clone /proc/sys/kernel/apparmor_restrict_unprivileged_userns /proc/sys/user/max_user_namespaces /proc/self/attr/current; do
    printf '\n%s\n' "$path"
    cat "$path" 2>/dev/null || true
  done
  sudo cat /sys/kernel/security/apparmor/profiles 2>/dev/null || true
  for path in /etc/apparmor.d/bwrap-userns-restrict /usr/share/apparmor/extra-profiles/bwrap-userns-restrict; do
    if [ -f "$path" ]; then
      printf '\nProfile %s\n' "$path"
      sha256sum "$path"
      cat "$path"
    fi
  done
}
bwrap_probe=(env -i PATH=/usr/bin:/bin /usr/bin/bwrap
  --unshare-user --unshare-pid --unshare-ipc --unshare-uts
  --disable-userns --cap-drop ALL --ro-bind /usr /usr
  --symlink usr/bin /bin --symlink usr/lib /lib --symlink usr/lib64 /lib64
  --proc /proc --dev /dev --tmpfs /tmp -- /bin/true)
probe() { "${bwrap_probe[@]}"; }
trace_probe() {
  strace -f -e trace=clone,clone3,unshare,capset,prctl,openat,write "${bwrap_probe[@]}"
}

record > "$evidence/before.txt" 2>&1
before=0
probe > "$evidence/probe-before.txt" 2>&1 || before=$?
trace_probe > "$evidence/trace-before.txt" 2>&1 || true
printf 'Initial prerequisite probe exit=%s\n' "$before"
cat "$evidence/before.txt" "$evidence/probe-before.txt" "$evidence/trace-before.txt"
# Load only the distro's Bubblewrap-specific userns policy. Its launcher may
# set up its own namespaces; its executed children remain capability-restricted.
# Do not disable AppArmor, alter its global userns restriction, create a generic
# unconfined profile, or grant outer capabilities to the test process.
profile=''
for candidate in /etc/apparmor.d/bwrap-userns-restrict /usr/share/apparmor/extra-profiles/bwrap-userns-restrict; do
  if [ -f "$candidate" ]; then profile="$candidate"; break; fi
done
if [ -z "$profile" ]; then
  mkdir -p "$evidence/packages" "$evidence/extracted"
  (
    cd "$evidence/packages"
    apt-get download apparmor-profiles
    for package in ./*.deb; do dpkg-deb --extract "$package" "$evidence/extracted"; done
  )
  profile="$evidence/extracted/usr/share/apparmor/extra-profiles/bwrap-userns-restrict"
fi
if [ ! -f "$profile" ]; then
  echo 'Distro Bubblewrap userns profile unavailable; prerequisites remain unsupported.' >&2
  exit 1
fi
cp "$profile" "$evidence/distro-bwrap-profile.txt"
sha256sum "$profile"
sudo apparmor_parser --replace "$profile"
after=0
probe > "$evidence/probe-after.txt" 2>&1 || after=$?
trace_probe > "$evidence/trace-after.txt" 2>&1 || true
record > "$evidence/after.txt" 2>&1
sudo journalctl --dmesg --boot --no-pager > "$evidence/kernel.txt" 2>&1 || true
cat "$evidence/after.txt" "$evidence/probe-after.txt" "$evidence/trace-after.txt"
rg 'apparmor|DENIED|userns|uid_map' "$evidence/kernel.txt" || true
printf 'After distro Bubblewrap policy probe exit=%s\n' "$after"
# Always run the real boundary suite in the next workflow step as well. This
# diagnostic never converts a missing prerequisite to passing test evidence.
exit "$after"
