import re
import unittest
from pathlib import Path


SKILL = Path(__file__).resolve().parents[1] / "SKILL.md"


class GithubAccountSuspensionAppealSkillTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = SKILL.read_text(encoding="utf-8")

    def test_frontmatter_is_discoverable(self):
        self.assertIn("name: github-account-suspension-appeal", self.text)
        description = re.search(r"^description: (.+)$", self.text, re.MULTILINE)
        self.assertIsNotNone(description)
        self.assertTrue(description.group(1).startswith("Use when "))
        for keyword in ("GitHub", "suspended", "restricted", "appeal"):
            self.assertIn(keyword, description.group(1))

    def test_core_workflow_sections_exist(self):
        for heading in (
            "## Intake",
            "## Form Fields",
            "## Writing Rules",
            "## Variation Rules",
            "## Output Contract",
            "## Common Mistakes",
        ):
            self.assertIn(heading, self.text)

    def test_required_appeal_facts_are_covered(self):
        for phrase in (
            "username",
            "student",
            "about two months",
            "Account suspended",
            "Terms of Service",
            "willing to verify",
        ):
            self.assertIn(phrase, self.text)

    def test_randomization_is_required_without_fabrication(self):
        for phrase in (
            "Do not reuse the same opening",
            "Rotate sentence order",
            "Do not invent",
            "Do not admit wrongdoing",
            "Do not claim exact dates unless provided",
        ):
            self.assertIn(phrase, self.text)

    def test_template_repetition_is_not_allowed(self):
        self.assertNotIn("[TODO", self.text)
        self.assertNotIn("copy this exact template", self.text.lower())
        self.assertIn("generate a fresh draft", self.text)


if __name__ == "__main__":
    unittest.main()
