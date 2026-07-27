import json
import os
import subprocess as sp

_INSTALLED_TOOLS = os.path.expanduser("~/.local/share/neuropipe/tools")
_BUNDLED_TOOLS = os.path.join(os.path.dirname(__file__), "..", "tools")
TOOLS_DIR = _INSTALLED_TOOLS if os.path.isdir(_INSTALLED_TOOLS) else _BUNDLED_TOOLS
EXEC_TIMEOUT = 30


class ToolDef:
    def __init__(self, name: str, filepath: str, metadata: dict):
        self.name = name
        self.filepath = filepath
        self.metadata = metadata

    def definition(self) -> dict:
        fn = self.metadata.get("function", self.metadata)
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": fn.get("description", ""),
                "parameters": fn.get("parameters", {"type": "object", "properties": {}}),
            },
        }

    def execute(self, params: dict) -> str:
        try:
            result = sp.run(
                [self.filepath, json.dumps(params)],
                capture_output=True, text=True, timeout=EXEC_TIMEOUT,
            )
            out = result.stdout.strip()
            if result.returncode != 0:
                return f"Error: tool exited with code {result.returncode}"
            try:
                data = json.loads(out)
            except json.JSONDecodeError:
                return out or "(empty output)"
            if data.get("success"):
                return data.get("result", "(ok)")
            else:
                return f"Error: {data.get('error', 'unknown')}"
        except sp.TimeoutExpired:
            return f"Error: tool timed out after {EXEC_TIMEOUT}s"
        except Exception as e:
            return f"Error: {e}"


class ToolManager:
    def __init__(self, initial_config: dict[str, str] | None = None):
        self._tools: dict[str, ToolDef] = {}
        self._config: dict[str, str] = dict(initial_config or {})
        self._granted: dict[str, bool] = {}

    def discover(self):
        if not os.path.isdir(TOOLS_DIR):
            return
        for name in sorted(os.listdir(TOOLS_DIR)):
            tool_dir = os.path.join(TOOLS_DIR, name)
            if not os.path.isdir(tool_dir):
                continue
            meta_file = os.path.join(tool_dir, "tool.json")
            exec_file = os.path.join(tool_dir, "run")
            if not os.path.isfile(meta_file) or not os.path.isfile(exec_file):
                continue
            try:
                with open(meta_file) as f:
                    metadata = json.load(f)
                tool = ToolDef(name, exec_file, metadata)
                self._tools[name] = tool
                if name not in self._config:
                    self._config[name] = metadata.get("default_permission", "ask")
            except Exception as e:
                print(f"[ToolManager] Failed to load '{name}': {e}")

    def active_definitions(self) -> list[dict]:
        return [
            tool.definition()
            for name, tool in self._tools.items()
            if self._config.get(name) != "deny"
        ]

    def list_all(self) -> dict[str, str]:
        return {name: self._config.get(name, "deny") for name in self._tools}

    def set_config(self, cfg: dict[str, str]):
        for name, level in cfg.items():
            if name in self._tools and level in ("allow", "ask", "deny"):
                self._config[name] = level
            elif name in self._tools:
                print(f"[ToolManager] Invalid level '{level}' for {name}")

    def check(self, name: str) -> str:
        return self._config.get(name, "deny")

    def is_granted(self, name: str) -> bool:
        return self._granted.get(name, False)

    def grant(self, name: str):
        self._granted[name] = True

    def reset_session(self):
        self._granted.clear()

    def execute(self, name: str, params: dict) -> str:
        tool = self._tools.get(name)
        if not tool:
            return f"Error: unknown tool '{name}'"
        return tool.execute(params)
