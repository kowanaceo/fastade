import base64
import json
import os
import sys

try:
    agent = sys.argv[1]
    if agent == "codex":
        path = os.path.join(os.environ.get("CODEX_HOME") or os.path.expanduser("~/.codex"), "auth.json")
        with open(path) as f:
            tokens = json.load(f).get("tokens", {})
        account_id = tokens.get("account_id")
        email = None
        try:
            part = tokens.get("id_token", "").split(".")[1]
            claims = json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))
            email = claims.get("email")
        except (ValueError, IndexError, TypeError, AttributeError):
            pass
    elif agent == "claude":
        with open(os.path.expanduser("~/.claude.json")) as f:
            account = json.load(f).get("oauthAccount", {})
        account_id = account.get("accountUuid")
        email = account.get("emailAddress")
    else:
        sys.exit(0)
    if isinstance(account_id, str) and account_id.strip() and len(account_id) <= 200:
        result = {"accountId": account_id.strip()}
        if isinstance(email, str) and email.strip() and len(email) <= 320:
            result["accountEmail"] = email.strip()
        print("ACCOUNT_JSON " + json.dumps(result))
except (OSError, ValueError, TypeError, AttributeError):
    pass
