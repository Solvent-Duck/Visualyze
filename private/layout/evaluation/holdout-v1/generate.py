#!/usr/bin/env python3
"""Emit the preselected synthetic holdout. No layout, scoring, or randomness."""
import hashlib
import json
from pathlib import Path

DIRECTORY = Path(__file__).resolve().parent
REPOSITORY = DIRECTORY.parents[3]


def claim():
    return {"basis": "declared", "evidence_ids": [], "rationale": None}


class Model:
    def __init__(self, name, description, profiles=(), format="bundle"):
        self.name = name
        self.description = description
        self.profiles = ["architecture.v1", *profiles]
        self.format = format
        self.root = f"scope:{name}"
        self.entities = {}
        self.relations = {}
        self.entity(self.root, "scope", description)

    def entity(self, id, kind, label=None, owner=None, ownership="contains", subtype=None):
        assert id not in self.entities
        self.entities[id] = {
            "summary": {"id": id, "kind": kind, "label": label or id.split(":", 1)[1].replace("-", " "),
                        "boundary": "internal", "claim": claim()},
            "detail": {"description": "Declared synthetic role; no measured production behavior is claimed.",
                       "subtype": subtype, "roles": [], "technology": []},
        }
        if owner is not None:
            self.relation(f"owns:{id}", ownership, owner, id)
        return id

    def add(self, kind, name, owner=None, **kwargs):
        return self.entity(f"{kind}:{name}", kind, owner=owner or self.root, **kwargs)

    def relation(self, id, kind, source, target, label=None, aspect=None):
        assert id not in self.relations
        self.relations[id] = {"id": id, "kind": kind, "source": source, "target": target,
                              "claim": claim(), "label": label, "aspect": aspect}

    def link(self, kind, source, target, label=None, suffix=""):
        self.relation(f"rel:{kind}:{source}:{target}{suffix}", kind, source, target, label,
                      "access_control" if kind == "applies_to" else None)

    def emit(self, policy, lens="logical", focus=None, expanded=()):
        omissions = [
            "Independently authored synthetic architecture, not a claim about a real system.",
            "Traffic volumes, failure handling, operational schedules, and unnamed dependencies are not supplied.",
            "No physical deployment is implied except explicit deployment records in this fixture.",
        ]
        entities = [self.entities[id] for id in sorted(self.entities)]
        relations = [self.relations[id] for id in sorted(self.relations)]
        if self.format == "bundle":
            filename = self.name + ".bundle.jsonl"
            manifest = {"format": "visualyze.bundle.v1", "model_id": f"holdout-v1:{self.name}",
                        "revision": 1, "root_id": self.root, "profiles": self.profiles,
                        "coverage": "partial", "scope_description": self.description,
                        "omissions": omissions}
            records = [{"record": "manifest", "value": manifest}]
            records += [{"record": "entity", "value": entity} for entity in entities]
            records += [{"record": "relation", "value": relation} for relation in relations]
            records += [{"record": "complete", "value": {"entities": len(entities), "relations": len(relations), "evidence": 0}}]
            content = "".join(json.dumps(record, separators=(",", ":")) + "\n" for record in records)
        else:
            filename = self.name + ".json"
            snapshot_entities = []
            for entity in entities:
                record = {key: value for key, value in entity["summary"].items() if key != "claim"}
                record["basis"] = "declared"
                if entity["detail"]["subtype"] is not None:
                    record["subtype"] = entity["detail"]["subtype"]
                if record["id"] == self.root:
                    # Snapshot system_id must name a scope with subtype system;
                    # bundle roots carry no subtype, so bundle bytes are unchanged.
                    record["subtype"] = "system"
                snapshot_entities.append(record)
            snapshot_relations = []
            for relation in relations:
                record = {key: value for key, value in relation.items() if key != "claim" and value is not None}
                record["basis"] = "declared"
                snapshot_relations.append(record)
            snapshot = {"schema_version": "0.1.0-draft.1", "model_id": f"holdout-v1:{self.name}",
                        "revision": 1, "system_id": self.root,
                        "snapshot": {"id": f"holdout-v1:{self.name}:revision-1", "source_revision": None,
                                     "coverage": "partial", "scope_description": self.description, "omissions": omissions},
                        "entities": snapshot_entities, "relations": snapshot_relations, "evidence": []}
            content = json.dumps(snapshot, indent=2) + "\n"
        path = DIRECTORY / filename
        with path.open("x") as output:
            output.write(content)
        return {
            "id": f"holdout-v1-{self.name}", "path": str(path.relative_to(REPOSITORY)),
            "format": self.format, "role": "held_out", "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "view": {"focus": focus or self.root, "expanded": sorted(expanded), "policy": policy,
                     "lens": lens if self.format == "bundle" else None, "child_page": 0, "relation_page": 0},
        }


def clinic():
    m = Model("clinic-appointments", "Synthetic clinic appointment and import responsibilities", format="snapshot")
    for name in ["public", "staff"]:
        m.add("client", name)
    services = ["gateway", "booking", "schedules", "identity", "reminders", "import"]
    for name in services:
        m.add("service", name)
    for name in ["accounts", "appointments", "availability", "archive"]:
        m.add("datastore", name)
    m.add("concern", "access", subtype="authentication")
    m.add("concern", "telemetry", subtype="observability")
    for name, owner in [("gateway-api", "gateway"), ("booking-api", "booking")]:
        m.add("interface", name, f"service:{owner}", ownership="exposes")
    m.add("interface", "records", "datastore:appointments", ownership="exposes")
    for client in ["public", "staff"]:
        m.link("requests", f"client:{client}", "interface:gateway-api")
    for service in ["booking", "schedules", "identity"]:
        m.link("requests", "service:gateway", f"service:{service}")
    for kind, service, store in [
        ("reads", "identity", "accounts"), ("writes", "identity", "accounts"),
        ("reads", "booking", "availability"), ("writes", "booking", "appointments"),
        ("writes", "schedules", "availability"), ("reads", "reminders", "appointments"),
        ("writes", "import", "archive"), ("writes", "import", "appointments"),
    ]:
        m.link(kind, f"service:{service}", f"datastore:{store}")
    m.link("reads", "service:booking", "interface:records")
    m.link("requests", "service:reminders", "service:identity")
    m.link("requests", "client:staff", "service:import")
    for service in ["gateway", "booking", "schedules"]:
        m.link("applies_to", "concern:access", f"service:{service}")
    m.link("implemented_by", "concern:access", "service:identity")
    for service in services:
        m.relation(f"observes:{service}", "applies_to", "concern:telemetry", f"service:{service}", aspect="observability")
    return m.emit("crud")


def research():
    m = Model("research-cooperative", "Synthetic cross-region research catalog with shared identity and audit")
    for scope in ["field", "catalog", "shared"]:
        m.add("scope", scope)
    for name, scope in [("collector", "field"), ("researcher", "catalog")]:
        m.add("client", name, f"scope:{scope}")
    owners = {"ingest": "field", "query": "field", "search": "catalog", "curation": "catalog",
              "export": "catalog", "identity": "shared", "entitlements": "shared", "metrics": "shared"}
    for name, scope in owners.items():
        m.add("service", name, f"scope:{scope}")
    for name, scope in [("samples", "field"), ("search-index", "catalog"), ("members", "shared"), ("audit", "shared")]:
        m.add("datastore", name, f"scope:{scope}")
    m.add("concern", "membership", subtype="authorization")
    m.link("requests", "client:collector", "service:ingest")
    m.link("requests", "client:researcher", "service:search")
    for service in ["ingest", "query", "search", "curation", "export"]:
        m.link("depends_on", f"service:{service}", "service:identity")
        m.link("writes", f"service:{service}", "datastore:audit")
    for service in ["query", "search", "export"]:
        m.link("requests", f"service:{service}", "service:entitlements")
        m.link("applies_to", "concern:membership", f"service:{service}")
    for service in ["identity", "entitlements"]:
        m.link("reads", f"service:{service}", "datastore:members")
    m.link("implemented_by", "concern:membership", "service:entitlements")
    for kind, source, target in [("writes", "ingest", "samples"), ("reads", "query", "samples"),
                                ("reads", "curation", "samples"), ("writes", "curation", "search-index"),
                                ("reads", "search", "search-index"), ("reads", "export", "samples"),
                                ("reads", "metrics", "audit")]:
        m.link(kind, f"service:{source}", f"datastore:{target}")
    m.link("requests", "service:search", "service:query")
    return m.emit("crud", expanded=["scope:field", "scope:catalog", "scope:shared"])


def parcel():
    m = Model("parcel-events", "Synthetic parcel event fan-out and repair feedback", ["messaging.v1"])
    for name in ["intake", "quote", "reserve", "audit", "dispatch", "status", "repair", "notifications"]:
        m.add("service", name)
    for name in ["submitted", "reserved", "delivered", "retry"]:
        m.add("message_channel", name, subtype="topic")
        m.add("message_type", name)
        m.link("carries", f"message_channel:{name}", f"message_type:{name}")
    for name in ["state", "audit-log"]:
        m.add("datastore", name)
    for service, channel in [("intake", "submitted"), ("reserve", "reserved"),
                             ("dispatch", "delivered"), ("repair", "retry")]:
        m.link("publishes", f"service:{service}", f"message_channel:{channel}")
    for channel, services in {"submitted": ["quote", "reserve", "audit", "dispatch"],
                              "reserved": ["dispatch", "status"],
                              "delivered": ["status", "audit", "notifications"],
                              "retry": ["intake"]}.items():
        for service in services:
            m.link("subscribes", f"service:{service}", f"message_channel:{channel}")
    m.link("requests", "service:status", "service:repair")
    m.link("requests", "service:quote", "service:reserve")
    m.link("writes", "service:audit", "datastore:audit-log")
    for service in ["reserve", "status"]:
        m.link("writes", f"service:{service}", "datastore:state")
    m.link("reads", "service:repair", "datastore:state")
    return m.emit("pipeline")


def museum():
    m = Model("museum-collections", "Synthetic collection and circulation tables with explicit cross-store references", ["data.v1"])
    m.add("client", "curator")
    for name in ["catalog", "circulation", "reporting"]:
        m.add("service", name)
    for name in ["catalog", "loans"]:
        m.add("datastore", name)
    tables = {"catalog": ["objects", "copies", "makers", "materials", "locations", "object-materials"],
              "loans": ["borrowers", "agreements", "loan-items", "events", "conditions", "charges"]}
    for store, names in tables.items():
        for name in names:
            m.add("table", name, f"datastore:{store}", ownership="owns_table")
    for column, table in [("copy-id", "copies"), ("loan-copy-id", "loan-items"),
                          ("agreement-id", "agreements"), ("loan-agreement-id", "loan-items")]:
        m.add("column", column, f"table:{table}", ownership="owns_column")
    for source, target in [("copies", "objects"), ("objects", "makers"), ("object-materials", "objects"),
                           ("object-materials", "materials"), ("copies", "locations"), ("agreements", "borrowers"),
                           ("loan-items", "agreements"), ("loan-items", "copies"), ("events", "loan-items"),
                           ("loan-items", "events"), ("conditions", "copies"), ("charges", "agreements")]:
        m.link("table_references", f"table:{source}", f"table:{target}")
    m.link("table_references", "column:loan-copy-id", "column:copy-id")
    m.link("table_references", "column:loan-agreement-id", "column:agreement-id")
    for service in ["catalog", "circulation", "reporting"]:
        m.link("requests", "client:curator", f"service:{service}")
    m.link("writes", "service:catalog", "table:objects")
    m.link("reads", "service:circulation", "table:copies")
    m.link("writes", "service:circulation", "table:loan-items")
    for table in ["objects", "agreements", "events"]:
        m.link("reads", "service:reporting", f"table:{table}")
    return m.emit("crud", expanded=["datastore:catalog", "datastore:loans"])


def compiler():
    m = Model("compiler-workspace", "Synthetic compiler ownership and shared IR source dependencies", ["code.v1"])
    m.add("code_repository", "compiler")
    expanded = []
    packages = ["parser", "ir", "optimizer", "driver"]
    for package in packages:
        package_id = m.add("code_package", package, "code_repository:compiler", ownership="owns_code")
        expanded.append(package_id)
        for suffix in ["core", "helpers"]:
            file = m.add("code_file", f"{package}-{suffix}", package_id, ownership="owns_code")
            expanded.append(file)
            owner = file
            if suffix == "core":
                owner = m.add("code_type", package, file, ownership="owns_code")
                expanded.append(owner)
            m.add("code_function", f"{package}-{suffix}", owner, ownership="owns_code")
        m.link("code_references", f"code_function:{package}-core", f"code_function:{package}-helpers")
    for source, target in [("driver-core", "parser-core"), ("driver-core", "optimizer-core"),
                           ("parser-core", "ir-core"), ("optimizer-core", "ir-core"),
                           ("optimizer-helpers", "ir-helpers"), ("driver-helpers", "ir-helpers"),
                           ("ir-helpers", "parser-helpers")]:
        m.link("code_references", f"code_function:{source}", f"code_function:{target}")
    m.link("code_references", "code_function:parser-helpers", "code_function:ir-helpers")
    return m.emit("code", lens="code", focus="code_repository:compiler", expanded=expanded)


def ledger():
    m = Model("regional-ledger", "Synthetic two-region ledger instances, separate from logical ownership", ["instances.v1"])
    m.add("client", "portal")
    for name in ["gateway", "ledger", "writer", "reconciler"]:
        m.add("service", name)
    for name in ["journal", "cache"]:
        m.add("datastore", name)
    m.entity("deployment:fleet", "deployment")
    expanded = []
    for region in ["east", "west"]:
        region_id = m.entity(f"deployment:{region}", "deployment")
        m.link("deployed_in", region_id, "deployment:fleet")
        expanded.append(region_id)
        for zone in ["a", "b"]:
            zone_id = m.entity(f"deployment:{region}-{zone}", "deployment")
            m.link("deployed_in", zone_id, region_id)
            expanded.append(zone_id)
        for name, prototype, zone in [("gateway", "service:gateway", "a"),
                                      ("ledger-a", "service:ledger", "a"),
                                      ("ledger-b", "service:ledger", "b"),
                                      ("writer", "service:writer", "b"),
                                      ("journal", "datastore:journal", "b"),
                                      ("cache", "datastore:cache", "a")]:
            instance = m.entity(f"deployment_instance:{region}-{name}", "deployment_instance")
            m.link("instance_of", instance, prototype)
            m.link("deployed_in", instance, f"deployment:{region}-{zone}")
    m.link("requests", "client:portal", "service:gateway")
    m.link("requests", "service:gateway", "service:ledger")
    m.link("requests", "service:ledger", "service:writer")
    m.link("writes", "service:writer", "datastore:journal")
    m.link("reads", "service:ledger", "datastore:cache")
    m.link("reads", "service:reconciler", "datastore:journal")
    return m.emit("deployment", lens="deployment", focus="deployment:fleet", expanded=expanded)


def islands():
    m = Model("islands-and-claims", "Synthetic disconnected runtime islands and distinct parallel request claims", format="snapshot")
    for name in ["crew", "inspector"]:
        m.add("client", name)
    for name in ["dispatch", "planner", "archive", "check", "offline", "warm", "spare", "standby"]:
        m.add("service", name)
    for name in ["assignments", "reports"]:
        m.add("datastore", name)
    m.add("concern", "crew-access", subtype="authorization")
    m.add("interface", "planning", "service:planner", ownership="exposes")
    m.add("interface", "standby", "service:standby", ownership="exposes")
    m.link("requests", "client:crew", "service:dispatch")
    m.link("requests", "service:dispatch", "interface:planning", label="Estimate assignment", suffix=":estimate")
    m.link("requests", "service:dispatch", "interface:planning", label="Reserve assignment", suffix=":reserve")
    m.link("reads", "service:planner", "datastore:assignments")
    m.link("writes", "service:planner", "datastore:assignments")
    m.link("applies_to", "concern:crew-access", "service:dispatch")
    m.link("requests", "client:inspector", "service:archive")
    m.link("requests", "service:archive", "service:check")
    m.link("requests", "service:check", "service:archive")
    m.link("reads", "service:archive", "datastore:reports")
    return m.emit("peers")


def water():
    m = Model("water-control", "Synthetic telemetry, analysis, and operations scopes with feedback", ["data.v1", "messaging.v1"])
    for scope in ["sensors", "analysis", "operations"]:
        m.add("scope", scope)
    for station in ["north", "central", "south"]:
        m.add("client", station, "scope:sensors")
    for name, scope in [("ingestor", "sensors"), ("forecast", "analysis"),
                         ("anomaly", "analysis"), ("operator", "operations")]:
        m.add("service", name, f"scope:{scope}")
    for channel, scope in [("telemetry", "sensors"), ("commands", "operations")]:
        m.add("message_channel", channel, f"scope:{scope}", subtype="topic")
        m.add("message_type", channel, f"scope:{scope}")
        m.link("carries", f"message_channel:{channel}", f"message_type:{channel}")
    m.add("datastore", "history", "scope:analysis")
    m.add("datastore", "configuration", "scope:operations")
    for table in ["samples", "stations", "forecasts", "alerts"]:
        m.add("table", table, "datastore:history", ownership="owns_table")
    m.add("concern", "operator-access", "scope:operations", subtype="authorization")
    for station in ["north", "central", "south"]:
        m.link("requests", f"client:{station}", "service:ingestor")
    m.link("publishes", "service:ingestor", "message_channel:telemetry")
    for service in ["forecast", "anomaly"]:
        m.link("subscribes", f"service:{service}", "message_channel:telemetry")
        m.link("reads", f"service:{service}", "table:samples")
    m.link("writes", "service:ingestor", "table:samples")
    m.link("writes", "service:forecast", "table:forecasts")
    m.link("writes", "service:anomaly", "table:alerts")
    for table in ["samples", "forecasts", "alerts"]:
        m.link("table_references", f"table:{table}", "table:stations")
    m.link("reads", "service:operator", "table:alerts")
    m.link("requests", "service:anomaly", "service:operator")
    m.link("publishes", "service:operator", "message_channel:commands")
    m.link("subscribes", "service:ingestor", "message_channel:commands")
    m.link("writes", "service:operator", "datastore:configuration")
    m.link("reads", "service:ingestor", "datastore:configuration")
    m.link("applies_to", "concern:operator-access", "service:operator")
    return m.emit("pipeline", expanded=["scope:sensors", "scope:analysis", "scope:operations", "datastore:history"])


if __name__ == "__main__":
    cases = [clinic(), research(), parcel(), museum(), compiler(), ledger(), islands(), water()]
    with (DIRECTORY / "cases.json").open("x") as output:
        json.dump({"cases": cases}, output, indent=2)
        output.write("\n")
    print(f"Created {len(cases)} unscored cases; semantic validation is still required.")
