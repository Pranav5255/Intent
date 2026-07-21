from __future__ import annotations

import unittest
from pathlib import Path


ROLE_A_ROOT = Path(__file__).resolve().parents[2]
UNIT_TREES = (ROLE_A_ROOT / "packaging" / "systemd", ROLE_A_ROOT / "packaging" / "debian" / "systemd")
COMPONENTS = (
    "intent-os-server.service",
    "intent-os-role-b.service",
    "intent-os-x11-tracker.service",
    "intent-os-workspace-watch.service",
)


class SystemdPackagingTests(unittest.TestCase):
    def test_backend_target_and_scheduler_wiring_is_identical_in_each_tree(self) -> None:
        for unit_tree in UNIT_TREES:
            target = (unit_tree / "intent-os-backend.target").read_text(encoding="utf-8")
            pipeline = (unit_tree / "intent-os-pipeline.service").read_text(encoding="utf-8")
            timer = (unit_tree / "intent-os-pipeline.timer").read_text(encoding="utf-8")
            self.assertIn("WantedBy=default.target", target)
            for unit in (*COMPONENTS, "intent-os-pipeline.timer"):
                self.assertIn(unit, target)
            self.assertIn("After=intent-os-server.service", pipeline)
            self.assertIn("Wants=intent-os-server.service", pipeline)
            self.assertIn("PartOf=intent-os-backend.target", pipeline)
            self.assertIn("intent_engine.scheduled_ingest", pipeline)
            self.assertIn("Environment=ENABLE_PIPELINE_TRIGGER=true", pipeline)
            self.assertIn("Environment=ROLE_B_DB_PATH=%h/.local/share/intent-os/intents.db", pipeline)
            self.assertIn("Unit=intent-os-pipeline.service", timer)
            self.assertIn("OnCalendar=*-*-* 00/3:00:00", timer)
            self.assertIn("Persistent=true", timer)
            self.assertIn("PartOf=intent-os-backend.target", timer)

    def test_components_use_backend_target_and_only_pipeline_enables_trigger(self) -> None:
        for unit_tree in UNIT_TREES:
            for unit_path in unit_tree.iterdir():
                content = unit_path.read_text(encoding="utf-8")
                self.assertNotIn("intent-os.target", content)
                if unit_path.name in COMPONENTS:
                    self.assertIn("PartOf=intent-os-backend.target", content)
                    self.assertIn("WantedBy=intent-os-backend.target", content)
                if unit_path.name != "intent-os-pipeline.service":
                    self.assertNotIn("Environment=ENABLE_PIPELINE_TRIGGER=true", content)
            role_b = (unit_tree / "intent-os-role-b.service").read_text(encoding="utf-8")
            self.assertIn("intent_engine.api:app --host 127.0.0.1 --port 9478", role_b)

    def test_debian_builder_installs_every_backend_unit(self) -> None:
        builder = (ROLE_A_ROOT / "packaging" / "build-deb.sh").read_text(encoding="utf-8")
        for unit in (*COMPONENTS, "intent-os-pipeline.service", "intent-os-pipeline.timer", "intent-os-backend.target"):
            self.assertIn(unit, builder)

    def test_debian_builder_includes_the_role_c_runtime_and_launcher(self) -> None:
        builder = (ROLE_A_ROOT / "packaging" / "build-deb.sh").read_text(encoding="utf-8")
        self.assertIn('ROLE_C_ROOT=$(cd "$ROOT/../role-c/app" && pwd)', builder)
        self.assertIn('npm --prefix "$ROLE_C_ROOT" run build', builder)
        self.assertIn('node_modules/electron/dist', builder)
        self.assertIn('intent-os-launcher', builder)
        self.assertIn('intent-os.desktop', builder)
        self.assertIn('icons/intent-os.svg', builder)
