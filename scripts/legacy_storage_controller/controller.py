"""Owning deployment controller for closed, journaled filesystem generations."""
import base64
import re

from protocol import (MAX_BODY, canonical, digest, member, plan as validate_plan,
                      require)


def catalogue_items(plan, entries, generation):
    items = []
    for entry in entries:
        root, path = entry["root"], entry["key"]
        key = path
        candidates = plan["members"]
        if root == "shared":
            require(any(value["staging_path"] == key for value in candidates),
                    "UnknownSharedPathOwner")
            kind = "local_staging"
        else:
            match = re.fullmatch(r"(\.attune-upload/[0-9]+)(?:#([0-9]+))?", path)
            if match:
                key = match[1]
                kind = "multipart" if match[2] else "remote_staging"
            else:
                # object_store::local creates exact {destination}#{integer}
                # temporary files. Ownership comes from the exclusive, complete
                # generation journal, NOT this filename convention.
                if not any(value["object_key"] == key for value in candidates):
                    key, marker, suffix = key.rpartition("#")
                    require(marker == "#" and suffix.isascii() and suffix.isdigit(),
                            "UnknownBlobPathOwner")
                require(any(value["object_key"] == key for value in candidates),
                        "UnknownBlobPathOwner")
                kind = "final_object"
        proof = {"generation": generation, "plan_digest": digest(plan),
                 "root": root, "physical_key": path, "identity": entry["identity"]}
        items.append({"kind": kind, "key": key,
                      "provider_identity": "inode:" + digest(proof),
                      "ownership": "owned", "ownership_proof": proof})
    items.sort(key=digest)
    require(len(canonical(items)) <= MAX_BODY, "CatalogueByteBound")
    return items


def current_inventory(original, current, deletion_started):
    """Validate every physical child, including crash-interrupted exact deletes."""
    expected = {(item["ownership_proof"]["root"], item["ownership_proof"]["physical_key"]): item
                for item in original}
    observed = {(entry["root"], entry["key"]): entry["identity"] for entry in current}
    require(set(observed) <= set(expected), "UncataloguedPhysicalChild")
    missing = set(expected) - set(observed)
    require(all(deletion_started(expected[key]) for key in missing), "UnconfirmedPhysicalAbsence")
    for key, value in observed.items():
        old = expected[key]["ownership_proof"]["identity"]
        if old == value:
            continue
        same_inode = lambda identity: (identity["device"], identity["inode"])
        removed_links = sum(same_inode(expected[absent]["ownership_proof"]["identity"])
                            == same_inode(old) for absent in missing)
        # Only our durable exact-link deletion intents may change ctime/nlink.
        # Content identity, size, mtime and the remaining link count must match.
        require(removed_links > 0
                and all(old[field] == value[field]
                        for field in ("device", "inode", "size", "mtime_ns"))
                and int(value["links"]) == int(old["links"]) - removed_links,
                "PhysicalIdentityChanged")
    return observed


class Controller:
    def __init__(self, config, journal, native):
        self.config, self.journal, self.native = config, journal, native

    def check_native(self):
        # Revalidate kernel/mount identity, not just a saved receipt echo.
        require(self.native.binding() == self.journal.binding(), "NativeGenerationChanged")

    def write(self, body, principal):
        require(principal["role"] == "writer", "WriterPrincipalRequired")
        require(set(body) == {"member", "mode", "payload_base64", "pause_after_first_chunk"},
                "InvalidWrite")
        require(type(body["pause_after_first_chunk"]) is bool, "InvalidPause")
        entry = member(body["member"])
        for key in ("native_instance", "worker_instance", "execution_attempt"):
            require(entry[key] == principal[key], "WriterIdentityMismatch")
        mode = body["mode"]
        require(mode in ("blob", "shared", "blob_and_shared"), "UnsupportedWriterMode")
        require((mode == "blob" and entry["object_key"] is not None and entry["staging_path"] is None)
                or (mode == "shared" and entry["staging_path"] is not None
                    and entry["object_key"] is None)
                or (mode == "blob_and_shared" and entry["object_key"] is not None
                    and entry["staging_path"] is not None), "UnlocatedWriteMode")
        try:
            payload = base64.b64decode(body["payload_base64"], validate=True)
        except (ValueError, TypeError):
            require(False, "InvalidPayloadEncoding")
        require(len(payload) <= 512 * 1024, "PayloadBound")
        self.check_native()
        self.native.reap(self.journal)
        self.journal.require_enrollment(entry, mode)
        # Admit-before-exec, never record secrets in the durable ledger.
        accepted = self.journal.admit(entry, mode, digest([body["payload_base64"], body["pause_after_first_chunk"]]))
        if accepted:
            self.native.launch(entry, mode, payload, body["pause_after_first_chunk"])
        return {"operation": entry["operation"], "accepted": True, "restarted": False}

    def prepare_origin(self, request):
        require(set(request) == {"nonce", "generation", "run_id", "project", "mode"}, "InvalidOriginRequest")
        require(request["generation"] == self.config["generation"]
                and request["mode"] in ("blob", "shared", "blob_and_shared"), "OriginScopeMismatch")
        require(re.fullmatch(r"[a-z][a-z0-9-]{0,19}", request["run_id"])
                and re.fullmatch(r"[a-z0-9_-]{1,128}", request["project"])
                and request["project"].startswith("eaccept-" + request["run_id"] + "-"), "InvalidOriginRun")
        self.check_native()
        self.native.exclusive_processes()
        require(not self.native.cgroup_read("cgroup.procs").strip()
                and self.native.cgroup_read("cgroup.freeze").strip() == "0" and not self.native.scan(),
                "OriginalGenerationNotPristine")
        writers = [(fingerprint, value) for fingerprint, value in self.config["principals"].items()
                   if value["role"] == "writer"]
        require(len(writers) == 1, "SingleOriginalWriterRequired")
        fingerprint, writer = writers[0]
        original = self.journal.enroll({"run_id": request["run_id"], "project": request["project"],
                                       "mode": request["mode"],
                                       "writer": {key: value for key, value in writer.items() if key != "role"},
                                       "certificate_sha256": fingerprint})
        return {"binding": self.journal.binding(), "enrollment": original}

    def stop(self, plan):
        self.check_native()
        self.journal.fence(plan)
        saved, _ = self.journal.audit(plan)
        if saved is not None:
            self.verify(plan, saved)
            return saved
        self.native.stop_and_drain(self.journal)
        binding = self.journal.binding()
        inventory = catalogue_items(plan, self.native.scan(), self.config["generation"])
        fence = {
            "scope": self.config["scope"], "plan_digest": digest(plan),
            **{output: sorted({value[field] for value in plan["members"]})
               for output, field in (("stopped_native_instances", "native_instance"),
                                     ("stopped_worker_instances", "worker_instance"),
                                     ("stopped_execution_attempts", "execution_attempt"))},
            "restart_fence": {"generation": self.config["generation"],
                              "binding_digest": digest(binding),
                              "barrier": "durable_irreversible_admission_fence",
                              "cgroup": binding["cgroup"], "populated": 0, "frozen": 1},
            "provider_request_settlement": {
                "provider": "linux_local_ext4_xfs", "request_digest": digest(plan["members"]),
                "accepted_requests": sorted(value["operation"] for value in plan["members"]),
                "barrier": "empty_owned_cgroup_then_successful_syncfs_both_roots"},
            "namespace_ownership": {"generation": self.config["generation"],
                                    "roots": binding["roots"], "inventory_digest": digest(inventory),
                                    "origin": "controller_created_root_inodes_before_first_admission"},
        }
        self.journal.settled(plan, fence, inventory)
        return fence

    def verify(self, plan, receipt):
        self.check_native()
        self.journal.fence(plan)
        saved, inventory = self.journal.audit(plan)
        require(saved is not None and receipt == saved, "FenceReceiptMismatch")
        self.native.require_frozen()
        self.native.stop_and_drain(self.journal)
        current_inventory(inventory, self.native.scan(),
                          lambda item: self.journal.deletion_started(plan, item))
        return saved

    def catalogue(self, plan, arguments):
        require(set(arguments) == {"fence", "cursor", "limit", "max_bytes"}, "InvalidCatalogue")
        self.verify(plan, arguments["fence"])
        _, items = self.journal.audit(plan)
        limit, budget = arguments["limit"], arguments["max_bytes"]
        require(type(limit) is int and 1 <= limit <= 128
                and type(budget) is int and 4096 <= budget <= 524288, "CatalogueBound")
        cursor = arguments["cursor"]
        start = 0
        if cursor is not None:
            require(isinstance(cursor, str) and len(cursor) <= 128, "InvalidCursor")
            index, separator, checksum = cursor.partition(":")
            require(separator == ":" and index.isascii() and index.isdigit(), "InvalidCursor")
            start = int(index)
            require(0 < start < len(items) and checksum == digest(items[:start]), "InvalidCursor")
        selected = []
        for item in items[start:start + limit]:
            if len(canonical(selected + [item])) > budget - 2048:
                break
            selected.append(item)
        require(bool(selected) or start == len(items), "CatalogueItemBound")
        end = start + len(selected)
        complete = end == len(items)
        return {"items": selected,
                "next_cursor": None if complete else f"{end}:{digest(items[:end])}",
                "complete": {"plan_digest": digest(plan), "fence_digest": digest(arguments["fence"]),
                             "proof": {"inventory_digest": digest(items), "count": len(items),
                                       "coverage": "entire_closed_native_generation"}}
                if complete else None}

    def delete(self, plan, arguments):
        require(set(arguments) == {"fence", "item"}, "InvalidDelete")
        self.verify(plan, arguments["fence"])
        item = arguments["item"]
        self.journal.delete_intent(plan, item)
        _, inventory = self.journal.audit(plan)
        current = current_inventory(inventory, self.native.scan(),
                                    lambda value: self.journal.deletion_started(plan, value))
        proof = item["ownership_proof"]
        expected = current.get((proof["root"], proof["physical_key"]))
        self.native.remove(item, expected)
        # Join/fence + syncfs and full exact inventory verification AGAIN after
        # deletion. A lost deletion response reuses the same durable intent.
        self.verify(plan, arguments["fence"])
        remaining = {(entry["root"], entry["key"]) for entry in self.native.scan()}
        require((proof["root"], proof["physical_key"]) not in remaining, "ExactDeletePending")
        self.journal.absent(plan, item)
        return {"item_digest": digest(item), "fence_digest": digest(arguments["fence"]),
                "proof": {"generation": self.config["generation"],
                          "exact_intent": digest(item), "barrier": "unlink_fsync_syncfs_under_fence"}}

    def management(self, request):
        require(set(request) == {"nonce", "plan", "operation", "arguments"}, "InvalidManagementRequest")
        plan = validate_plan(request["plan"], self.config["scope"])
        operation, arguments = request["operation"], request["arguments"]
        if operation == "stop-and-settle":
            require(arguments == {}, "InvalidStop")
            return self.stop(plan)
        if operation == "verify-fence":
            require(set(arguments) == {"receipt"}, "InvalidVerify")
            return self.verify(plan, arguments["receipt"])
        if operation == "catalogue":
            return self.catalogue(plan, arguments)
        require(operation == "delete-exact", "UnsupportedManagementOperation")
        return self.delete(plan, arguments)
