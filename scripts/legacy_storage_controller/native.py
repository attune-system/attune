"""Linux cgroup-v2 + local ext4/XFS provider, not an S3 management shim."""
import ctypes
import fcntl
import hashlib
import os
from pathlib import Path
import signal
import stat
import subprocess
import time

from protocol import MAX_BODY, MAX_ITEMS, canonical, digest, relative, require

LIBC = ctypes.CDLL(None, use_errno=True)
DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


def directory(path):
    require(os.path.isabs(path), "AbsoluteRootRequired")
    fd = os.open("/", DIRECTORY)
    try:
        for part in path.split("/")[1:]:
            if part:
                require(part not in (".", ".."), "InvalidRoot")
                new = os.open(part, DIRECTORY, dir_fd=fd)
                os.close(fd)
                fd = new
        return fd
    except BaseException:
        os.close(fd)
        raise


def fs_type(fd):
    buffer = (ctypes.c_long * 64)()
    require(LIBC.fstatfs(fd, ctypes.byref(buffer)) == 0, "FilesystemUnavailable")
    return buffer[0]


def inode(value):
    # Strings preserve full native u64 identities through JCS/IEEE754 clients.
    return {"device": str(value.st_dev), "inode": str(value.st_ino),
            "ctime_ns": str(value.st_ctime_ns), "mtime_ns": str(value.st_mtime_ns),
            "size": str(value.st_size), "links": str(value.st_nlink)}


def root_identity(fd):
    value = os.fstat(fd)
    # ctime changes on child creation, so it is NOT root identity.
    mount = next(line.split(":", 1)[1].strip()
                 for line in Path(f"/proc/self/fdinfo/{fd}").read_text().splitlines()
                 if line.startswith("mnt_id:"))
    return {"device": str(value.st_dev), "inode": str(value.st_ino), "mount": mount,
            "filesystem": str(fs_type(fd))}


class Native:
    def __init__(self, config, initialize=False):
        self.config = config
        require(os.getuid() == os.geteuid() == config["service_uid"] > 0,
                "DedicatedUnprivilegedServiceRequired")
        require(os.getgid() == os.getegid() > 0 and set(os.getgroups()) <= {os.getegid()},
                "DedicatedCredentialGroupRequired")
        status = Path("/proc/self/status").read_text().splitlines()
        require(all(int(line.split()[1], 16) == 0 for line in status
                    if line.split(":", 1)[0] in ("CapInh", "CapPrm", "CapEff", "CapAmb")),
                "UnprivilegedControllerRequired")
        require(LIBC.prctl(38, 1, 0, 0, 0) == 0, "NoNewPrivilegesUnavailable")
        require(os.readlink("/proc/self/ns/pid") == config["host_pid_namespace"],
                "HostProcessInventoryUnavailable")
        proc = [line.split() for line in Path("/proc/self/mountinfo").read_text().splitlines()
                if line.split()[4] == "/proc"]
        require(len(proc) == 1 and not any(option.startswith("hidepid=") and option != "hidepid=0"
                                         for field in proc[0] for option in field.split(",")),
                "CompleteProcessInventoryRequired")
        paths = [Path(config["roots"][name]) for name in ("blob", "shared")]
        require(paths[0] != paths[1], "OverlappingRoots")
        require(not any(a in b.parents for a in paths for b in paths if a != b),
                "OverlappingRoots")
        state = Path(config["state_dir"])
        require(all(root != state and root not in state.parents and state not in root.parents
                    for root in paths), "JournalInsideWriterNamespace")
        self.cgroup = directory(config["cgroup"])
        require(fs_type(self.cgroup) == 0x63677270, "CgroupV2Required")
        require(not any(entry.is_dir(follow_symlinks=False)
                        for entry in os.scandir(self.cgroup)), "CgroupChildrenForbidden")
        self.children = {}
        self.exclusive_processes()
        if initialize:
            require(not self.cgroup_read("cgroup.procs").strip(), "UntrackedHistoricalWriter")
            require(self.cgroup_read("cgroup.freeze").strip() == "0", "FreshWriterCgroupRequired")
            for path in paths:
                parent = directory(str(path.parent))
                try:
                    value = os.fstat(parent)
                    require(value.st_uid == config["service_uid"]
                            and stat.S_IMODE(value.st_mode) == 0o700, "PrivateRootParentRequired")
                    # New inode origin, not an empty-directory observation. An
                    # existing empty root may still have old open descriptors.
                    require(not os.path.lexists(path), "UntrackedHistoricalRoot")
                    os.mkdir(path.name, 0o700, dir_fd=parent)
                    os.fsync(parent)
                finally:
                    os.close(parent)
        self.roots = {name: directory(config["roots"][name]) for name in ("blob", "shared")}
        require((os.fstat(self.roots["blob"]).st_dev, os.fstat(self.roots["blob"]).st_ino)
                != (os.fstat(self.roots["shared"]).st_dev, os.fstat(self.roots["shared"]).st_ino),
                "OverlappingRoots")
        for fd in self.roots.values():
            value = os.fstat(fd)
            require(value.st_uid == config["service_uid"] and stat.S_IMODE(value.st_mode) == 0o700,
                    "ExclusiveRootOwnershipRequired")
            require(fs_type(fd) in (0xEF53, 0x58465342), "ProviderCannotAttestSettlement")

    def binding(self):
        for name, original in self.roots.items():
            current = directory(self.config["roots"][name])
            try:
                require(root_identity(current) == root_identity(original), "NativeRootReplaced")
                value = os.fstat(current)
                require(value.st_uid == self.config["service_uid"]
                        and stat.S_IMODE(value.st_mode) == 0o700, "RootExclusionLost")
            finally:
                os.close(current)
        current = directory(self.config["cgroup"])
        try:
            require(root_identity(current) == root_identity(self.cgroup), "WriterCgroupReplaced")
        finally:
            os.close(current)
        return {"generation": self.config["generation"], "scope": self.config["scope"],
                "roots": {name: root_identity(fd) for name, fd in self.roots.items()},
                "cgroup": root_identity(self.cgroup),
                "host_pid_namespace": self.config["host_pid_namespace"],
                "boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                "writer_sha256": self.config["writer_sha256"],
                "service_uid": self.config["service_uid"], "service_gid": os.getegid()}

    def cgroup_read(self, key):
        fd = os.open(key, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.cgroup)
        try:
            value = os.read(fd, 65537)
            require(len(value) <= 65536, "CgroupBound")
            return value.decode("ascii")
        finally:
            os.close(fd)

    def exclusive_processes(self):
        members = set(map(int, self.cgroup_read("cgroup.procs").split()))
        require(os.getpid() not in members, "ControllerInsideWriterCgroup")
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                status = (entry / "status").read_text()
            except FileNotFoundError:
                continue
            ids = next(line.split()[1:] for line in status.splitlines() if line.startswith("Uid:"))
            if any(line.startswith("State:\tZ") for line in status.splitlines()):
                continue
            groups = next(line.split()[1:] for line in status.splitlines() if line.startswith("Gid:"))
            groups += next(line.split()[1:] for line in status.splitlines() if line.startswith("Groups:"))
            require(str(os.getegid()) not in groups or str(self.config["service_uid"]) in ids or "0" in ids,
                    "NativeCredentialGroupShared")
            if str(self.config["service_uid"]) in ids:
                require(int(entry.name) == os.getpid() or int(entry.name) in members,
                        "UncontrolledWriterProcess")

    def reap(self, journal):
        for operation, child in list(self.children.items()):
            if child.poll() is not None:
                child.wait()
                journal.joined(operation)
                del self.children[operation]

    def launch(self, entry, mode, payload, pause):
        self.exclusive_processes()
        path = self.config["writer_binary"]
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            value = os.fstat(fd)
            require(stat.S_ISREG(value.st_mode) and value.st_uid == 0
                    and value.st_mode & (0o022 | stat.S_ISUID | stat.S_ISGID) == 0,
                    "UntrustedWriterBinary")
            with os.fdopen(os.dup(fd), "rb") as stream:
                require(hashlib.file_digest(stream, "sha256").hexdigest() == self.config["writer_sha256"],
                        "WriterBinaryChanged")
            root = self.config["roots"]["shared" if mode == "shared" else "blob"]
            key = entry["staging_path"] if mode == "shared" else entry["object_key"]
            header = canonical({"root": root, "key": key, "mode": mode,
                                "shared_root": self.config["roots"]["shared"],
                                "staging_path": entry["staging_path"] if mode == "blob_and_shared" else None,
                                "length": len(payload), "pause_after_first_chunk": pause}) + b"\n"
            require(len(header) <= 8192, "WriterHeaderBound")
            parent = os.getpid()

            def attach_before_exec():
                # A parent death during fork/exec cannot leave an unattached
                # writer. Check ppid AFTER arming the kernel death signal.
                if LIBC.prctl(1, signal.SIGKILL, 0, 0, 0) != 0 or os.getppid() != parent:
                    os._exit(125)
                target = os.open("cgroup.procs", os.O_WRONLY | os.O_NOFOLLOW,
                                 dir_fd=self.cgroup)
                os.write(target, str(os.getpid()).encode("ascii"))
                os.close(target)

            body = os.memfd_create("native-write", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
            try:
                with os.fdopen(os.dup(body), "wb") as stream:
                    stream.write(header + payload)
                os.lseek(body, 0, os.SEEK_SET)
                fcntl.fcntl(body, fcntl.F_ADD_SEALS,
                            fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK
                            | fcntl.F_SEAL_SEAL)
                child = subprocess.Popen([f"/proc/self/fd/{fd}"], stdin=body,
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                         env={}, close_fds=True, pass_fds=(fd, self.cgroup),
                                         preexec_fn=attach_before_exec)
                self.children[entry["operation"]] = child
            finally:
                os.close(body)
        finally:
            os.close(fd)

    def stop_and_drain(self, journal):
        self.exclusive_processes()
        fd = os.open("cgroup.freeze", os.O_WRONLY | os.O_NOFOLLOW, dir_fd=self.cgroup)
        try:
            os.write(fd, b"1")
        finally:
            os.close(fd)
        deadline = time.monotonic() + 3
        while "frozen 1" not in self.cgroup_read("cgroup.events").splitlines():
            require(time.monotonic() < deadline, "WriterFreezePending")
            time.sleep(0.01)
        fd = os.open("cgroup.kill", os.O_WRONLY | os.O_NOFOLLOW, dir_fd=self.cgroup)
        try:
            os.write(fd, b"1")
        finally:
            os.close(fd)
        deadline = time.monotonic() + 3
        while "populated 0" not in self.cgroup_read("cgroup.events").splitlines():
            require(time.monotonic() < deadline, "WriterDrainPending")
            # Deadline limits a step; elapsed time never proves settlement.
            time.sleep(0.01)
        for operation, child in list(self.children.items()):
            child.wait(timeout=1)
            journal.joined(operation)
        self.children.clear()
        self.exclusive_processes()
        for fd in self.roots.values():
            require(LIBC.syncfs(fd) == 0, "ProviderDrainPending")
        self.require_frozen()
        require(not self.cgroup_read("cgroup.procs").strip(), "WriterJoinPending")

    def require_frozen(self):
        require(self.cgroup_read("cgroup.freeze").strip() == "1"
                and "frozen 1" in self.cgroup_read("cgroup.events").splitlines(),
                "RestartFenceLost")

    def scan(self):
        result = []
        visited = 0
        for name, root in self.roots.items():
            def walk(fd, prefix, depth):
                nonlocal visited
                require(depth <= 16, "InventoryDepthBound")
                entries = []
                with os.scandir(fd) as iterator:
                    for entry in iterator:
                        visited += 1
                        require(visited <= MAX_ITEMS, "InventoryBound")
                        entries.append(entry)
                for entry in sorted(entries, key=lambda entry: entry.name):
                    key = relative(prefix + entry.name)
                    value = entry.stat(follow_symlinks=False)
                    require(value.st_dev == os.fstat(root).st_dev, "NestedProvider")
                    if stat.S_ISDIR(value.st_mode):
                        child = os.open(entry.name, DIRECTORY, dir_fd=fd)
                        try:
                            walk(child, key + "/", depth + 1)
                        finally:
                            os.close(child)
                    else:
                        require(stat.S_ISREG(value.st_mode), "UnownedPhysicalAlias")
                        result.append({"root": name, "key": key, "identity": inode(value)})
                        require(len(result) <= MAX_ITEMS, "InventoryBound")
            walk(root, "", 0)
        groups = {}
        for entry in result:
            key = (entry["identity"]["device"], entry["identity"]["inode"])
            groups.setdefault(key, []).append(entry)
        for entries in groups.values():
            require(all(int(entry["identity"]["links"]) == len(entries) for entry in entries),
                    "UnownedPhysicalAlias")
        require(len(canonical(result)) <= MAX_BODY // 2, "InventoryByteBound")
        return result

    def parent(self, root, key):
        parts = relative(key).split("/")
        fd = os.dup(self.roots[root])
        try:
            for part in parts[:-1]:
                child = os.open(part, DIRECTORY, dir_fd=fd)
                os.close(fd)
                fd = child
            return fd, parts[-1]
        except BaseException:
            os.close(fd)
            raise

    def remove(self, item, expected):
        proof = item["ownership_proof"]
        try:
            fd, leaf = self.parent(proof["root"], proof["physical_key"])
        except FileNotFoundError:
            # Only callable AFTER a matching durable exact-delete intent.
            return
        try:
            try:
                current = os.stat(leaf, dir_fd=fd, follow_symlinks=False)
            except FileNotFoundError:
                os.fsync(fd)
                return
            require(inode(current) == expected and stat.S_ISREG(current.st_mode),
                    "PhysicalIdentityChanged")
            os.unlink(leaf, dir_fd=fd)
            os.fsync(fd)
        finally:
            os.close(fd)
        require(LIBC.syncfs(self.roots[proof["root"]]) == 0, "ProviderDrainPending")
