"""Desktop team-server authorization, corresponding to the application's firestore.rules.

The request UID comes only from a verified Firebase ID token. Reads use the
current database snapshot. Every write in a batch sees the same original
snapshot and the same prospective final snapshot.
"""

import json
import re


def _segment(value):
    return isinstance(value, str) and bool(value) and "/" not in value and value not in (".", "..")


def _path(path):
    parts = path.split("/") if isinstance(path, str) else []
    return parts if all(_segment(part) for part in parts) else []


def _text(value, limit, minimum=0):
    return isinstance(value, str) and minimum <= len(value) <= limit


def _only(document, keys):
    return document.keys() <= set(keys.split())


def _unchanged_except(before, after, *keys):
    allowed = set(keys)
    # JSON-typed equality: in Python, True compares equal to 1, including in maps.
    def kept(doc):
        return json.dumps({k: v for k, v in doc.items() if k not in allowed}, sort_keys=True)

    return kept(before) == kept(after)


def _closed(doc):
    return doc is not None and doc.keys() == {"closed"} and doc["closed"] is True


def _topic(doc):
    return (_text(doc.get("topic"), 40, 1)
            and re.fullmatch(r"[a-z_]+", doc["topic"]) is not None
            and _text(doc.get("week"), 8, 8)
            and re.fullmatch(r"[0-9]{4}-W[0-9]{2}", doc["week"]) is not None)


def _url(value):
    return _text(value, 500) and re.fullmatch(r"https://.*", value) is not None


class _Team:
    def __init__(self, uid, team_id, get):
        self.uid = uid
        self.path = "teams/" + team_id
        self.get = get
        self.root = get(self.path)

    def doc(self, collection, key, lookup=None):
        if not _segment(key):
            return None
        return (lookup or self.get)(f"{self.path}/{collection}/{key}")

    @property
    def open(self):
        return self.root is not None and self.root.get("closed", False) is False

    @property
    def member(self):
        return self.open and self.doc("members", self.uid) is not None

    @property
    def admin(self):
        return self.open and self.doc("admins", self.uid) is not None


def can_read(uid, path, get):
    """Authorize a single document read, including checks for missing documents."""
    parts = _path(path)
    if not _segment(uid) or len(parts) < 2 or len(parts) % 2:
        return False
    group, team_id, *sub = parts
    if group == "joinCodes":
        return not sub
    if group != "teams":
        return False
    team = _Team(uid, team_id, get)
    if not team.open:
        return False
    match sub:
        case []:
            return team.root.get("createdBy") == uid or team.member
        case ["members", who]:
            return who == uid or team.member
        case ["names", _]:
            return True
        case ["admins", who]:
            return who == uid
        case ["admin", "config"] | ["adminKeys", _] | ["signals", _]:
            return team.member
        case ["profiles" | "projects" | "shares" | "integrations" | "telegram" | "proposals", _]:
            return team.member
        case ["projects", _, "tasks" | "files", _]:
            return team.member
        case ["projects", project, "briefs", "current"]:
            return team.member and team.doc("projects", project) is not None
        case ["signals", signal, "private", "body"]:
            if not team.member:
                return False
            recipient = (get(path) or {}).get("toMember")
            owner = get(f"{team.path}/signals/{signal}/owner/sender") or {}
            return recipient == uid or owner.get("uid") == uid
        case ["signals", _, "owner", "sender"]:
            return team.member and (get(path) or {}).get("uid") == uid
    return False


def can_list(uid, collection, get, filters=()):
    """The signal inbox requires an exact predicate. Never enumerate private children."""
    parts = _path(collection)
    if not _segment(uid) or len(parts) < 3 or parts[0] != "teams":
        return False
    if not _Team(uid, parts[1], get).member:
        return False
    match parts[2:]:
        case ["members" | "names" | "profiles" | "projects" | "shares" | "integrations" | "telegram" | "proposals"]:
            return True
        case ["projects", _, "files" | "tasks"]:
            return True
        case ["signals"]:
            return ("toMember", "EQUAL", uid) in filters
    return False


def _join_code(uid, code, before, after, get, get_after):
    if after is None:
        return False
    team_id = (before if before is not None else after).get("teamId")
    if not _segment(team_id):
        return False
    path = f"teams/{team_id}"
    team = get(path) or {}
    if team.get("createdBy") != uid or team.get("joinCode") != code:
        return False
    if before is None:
        return _only(after, "teamId teamName")
    return _closed(after) and _closed(get_after(path))


def _member_write(team, who, before, after, get_after):
    if after is None:
        return who == team.uid or team.admin
    if who != team.uid or not _text(after.get("displayName"), 30):
        return False
    name = team.doc("names", after.get("nameKey"), get_after) or {}
    if name.get("uid") != who:
        return False
    if before is None:
        return (_only(after, "displayName joinCode nameKey") and _segment(after.get("joinCode"))
                and after["joinCode"] == team.root.get("joinCode"))
    return _unchanged_except(before, after, "displayName", "nameKey")


def _signal_write(team, sub, before, after, get_after):
    if not team.member:
        return False
    signal = f"{team.path}/signals/{sub[1]}"
    body, owner = signal + "/private/body", signal + "/owner/sender"
    match sub[2:]:
        case []:
            if after is None:
                return ((team.get(owner) or {}).get("uid") == team.uid
                        and get_after(body) is None and get_after(owner) is None)
            if before is None:
                return (_only(after, "topic toMember week held") and _topic(after)
                        and after.get("held") is False and after.get("toMember") != team.uid
                        and team.doc("members", after.get("toMember")) is not None)
            return (before.get("toMember") == team.uid and after.get("held") is True
                    and _unchanged_except(before, after, "held"))
        case ["owner", "sender"] | ["private", "body"]:
            if after is None:
                return (team.get(owner) or {}).get("uid") == team.uid and get_after(signal) is None
            if before is not None or team.get(signal) is not None:
                return False
            if sub[2] == "owner":
                return after == {"uid": team.uid} and get_after(body) is not None
            recipient = after.get("toMember")
            return (_segment(recipient) and (get_after(signal) or {}).get("toMember") == recipient
                    and (get_after(owner) or {}).get("uid") == team.uid and _only(after, "toMember title text")
                    and _text(after.get("title"), 80, 1) and _text(after.get("text"), 400, 1))
    return False


def _project_write(team, sub, before, after):
    if not team.member:
        return False
    project = team.doc("projects", sub[1])

    def can_remove():
        return project is not None and (project.get("createdBy") == team.uid or team.admin)

    def removing():
        return project is not None and project.get("deleting") is True and can_remove()

    children = sub[2:]
    if not children:
        if after is None:
            return removing()
        if not (_only(after, "title goal deliverable deadline createdBy deleting") and _text(after.get("title"), 80)):
            return False
        if before is None:
            return "deleting" not in after and after.get("createdBy", team.uid) == team.uid
        if after.get("createdBy", "") != before.get("createdBy", ""):
            return False
        if "deleting" not in after:
            return before.get("deleting", False) is False
        return after["deleting"] is True and can_remove() and _unchanged_except(before, after, "deleting")

    if len(children) != 2:
        return False
    collection, child = children
    if after is None:
        if collection == "tasks":
            return True
        own = before is not None and before.get("by") == team.uid
        if collection == "briefs" and child == "current":
            return own or team.admin or removing()
        return collection == "files" and (own or removing())
    if project is None or project.get("deleting", False) is not False:
        return False
    if collection == "briefs" and child == "current":
        return (_only(after, "name text by updatedAt") and after.get("by") == team.uid
                and type(after.get("updatedAt")) is int and _text(after.get("name"), 200)
                and re.fullmatch(r".+\.(?:[mM][dD]|[tT][xX][tT])", after["name"]) is not None
                and _text(after.get("text"), 49152, 1) and len(after["text"].encode("utf-8")) <= 49152)
    if collection == "tasks":
        accepted = after.get("accepted", False)
        keep_acceptance = (before is not None and before.get("accepted") is True
                           and "assignee" in before and before["assignee"] == after.get("assignee"))
        return (_only(after, "title estimateHours difficulty skills dependsOn doneCriteria prep reviewer assignee accepted status startedAt doneAt objections")
                and _text(after.get("title"), 80) and after.get("status") in ("todo", "doing", "blocked", "done")
                and (accepted is False or after.get("assignee") == team.uid or keep_acceptance))
    if collection == "files":
        if before is None:
            return (_only(after, "name url driveId mimeType size taskId access kind by at")
                    and after.get("by") == team.uid and _text(after.get("name"), 200) and _url(after.get("url")))
        return (before.get("by") == team.uid and _unchanged_except(before, after, "access")
                and after.get("access") in ("edit", "view"))
    return False


def can_bootstrap(uid, prospective, get):
    """Only an entire, previously absent team + code + name + creator membership can initialize a room."""
    if not _segment(uid) or len(prospective) != 4:
        return False
    roots = [p for p in prospective if len(_path(p)) == 2 and p.startswith("teams/")]
    if len(roots) != 1 or any(get(path) is not None for path in prospective):
        return False
    root_path = roots[0]
    root = prospective[root_path]
    if not isinstance(root, dict) or set(root) != {"name", "joinCode", "createdBy"}:
        return False
    code, name = root["joinCode"], root["name"]
    if (root["createdBy"] != uid or not _text(name, 60, 1)
            or not _text(code, 6, 6) or not re.fullmatch(r"[ABCDEFGHJKLMNPQRSTUVWXYZ23456789]{6}", code)):
        return False
    code_path = "joinCodes/" + code
    if prospective.get(code_path) != {"teamId": root_path.split("/")[1], "teamName": name}:
        return False
    member = prospective.get(f"{root_path}/members/{uid}")
    if not isinstance(member, dict) or set(member) != {"displayName", "nameKey", "joinCode"}:
        return False
    key = member["nameKey"]
    return (member["joinCode"] == code and _text(member["displayName"], 30, 1) and _segment(key)
            and prospective.get(f"{root_path}/names/{key}") == {"uid": uid})


def can_write(uid, path, before, after, get, get_after):
    """Authorize exactly one document as part of its atomic batch."""
    parts = _path(path)
    if not _segment(uid) or len(parts) < 2 or len(parts) % 2:
        return False
    if any(doc is not None and not isinstance(doc, dict) for doc in (before, after)):
        return False
    group, team_id, *sub = parts
    if group == "joinCodes" and not sub:
        return _join_code(uid, team_id, before, after, get, get_after)
    if group != "teams":
        return False
    team = _Team(uid, team_id, get)
    if not sub:
        if after is None:
            return False
        if before is None:
            # A new desktop room is authorized by can_bootstrap at the batch boundary.
            return False
        code = before.get("joinCode")
        return (team.member and before.get("createdBy") == uid and _closed(after) and _segment(code)
                and (get("joinCodes/" + code) or {}).get("teamId") == team_id
                and _closed(get_after("joinCodes/" + code)))
    if not team.open:
        return False
    if sub[0] == "signals":
        return _signal_write(team, sub, before, after, get_after)
    if sub[0] == "projects":
        return _project_write(team, sub, before, after)
    if len(sub) != 2:
        return False
    creation = before is None and after is not None
    collection, key = sub
    if collection == "members":
        return _member_write(team, key, before, after, get_after)
    if collection == "names":
        if after is None:
            who = (before or {}).get("uid")
            return who == uid or team.admin or (_segment(who) and team.member and team.doc("members", who) is None)
        return creation and after == {"uid": uid} and team.doc("members", uid, get_after) is not None
    if after is None:
        if collection == "admins" or collection == "profiles":
            return key == uid or team.admin
        if collection == "telegram":
            return key == uid
    if not team.member:
        return False
    if collection == "admin" and key == "config":
        return (creation and _only(after, "adminId salt by") and after.get("by") == uid
                and _text(after.get("adminId"), 30))
    if collection == "adminKeys":
        return (creation and team.doc("admin", "config") is None
                and (team.doc("admin", "config", get_after) or {}).get("by") == uid)
    if collection == "admins":
        return (creation and key == uid and _only(after, "key")
                and team.doc("adminKeys", after.get("key"), get_after) is not None)
    if collection == "profiles":
        hours = after.get("weeklyHours", 0)
        return (key == uid and _only(after, "pr skills learn weeklyHours days") and _text(after.get("pr", ""), 500)
                and type(hours) in (float, int) and 0 <= hours <= 80)
    if collection == "shares":
        if after is None:
            return (before or {}).get("by") == uid
        return (creation and _only(after, "text name url driveId mimeType size access by at")
                and after.get("by") == uid and _text(after.get("text"), 1000)
                and ("url" not in after or _url(after["url"])))
    if collection == "integrations":
        if after is None:
            return key != "claude" or (before or {}).get("sharedBy") == uid
        if key == "telegram":
            return _only(after, "botToken botUsername") and _text(after.get("botToken"), 100)
        return (key == "claude" and _only(after, "apiKey sharedBy") and _text(after.get("apiKey"), 300)
                and after.get("sharedBy") == uid and (creation or before.get("sharedBy") == uid))
    if collection == "telegram":
        return (key == uid and _only(after, "chatId teamAlerts") and type(after.get("chatId")) is int
                and type(after.get("teamAlerts")) is bool)
    if collection == "proposals":
        if after is None:
            post = f"{team.path}/shares/{key}"
            return ((before or {}).get("topic") == "ai_promise" and (get(post) or {}).get("by") == uid
                    and get_after(post) is None)
        if creation:
            return _only(after, "topic week responses") and _topic(after) and after.get("responses") == {}
        votes, updated = before.get("responses"), after.get("responses")
        return (isinstance(votes, dict) and isinstance(updated, dict) and _unchanged_except(before, after, "responses")
                and _unchanged_except(votes, updated, uid) and updated.get(uid) in ("agree", "later"))
    return False
