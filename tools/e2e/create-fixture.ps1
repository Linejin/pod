# SPDX-License-Identifier: Apache-2.0
. (Join-Path $PSScriptRoot 'session.ps1')
$sample = Join-Path $E2eRoot 'sample'
if (Test-Path $sample) { throw "Refusing to overwrite existing fixture: $sample" }
New-Item -ItemType Directory $sample | Out-Null
Set-Content -Encoding UTF8 (Join-Path $sample 'calc.py') "def add(a, b):`n    return a - b`n"
Set-Content -Encoding UTF8 (Join-Path $sample 'test_calc.py') "import unittest`nfrom calc import add`n`nclass CalcTest(unittest.TestCase):`n    def test_add(self):`n        self.assertEqual(add(2, 3), 5)`n`nif __name__ == '__main__':`n    unittest.main()`n"
Set-Content -Encoding UTF8 (Join-Path $sample 'README.md') "# Pod E2E fixture`nRun: python -m unittest -v`n"
Set-Content -Encoding UTF8 (Join-Path $sample '.gitignore') '__pycache__/'
& git -C $sample init -b main
& git -C $sample config user.name 'Pod E2E'
& git -C $sample config user.email 'pod-e2e@example.invalid'
& git -C $sample add .
& git -C $sample commit -m 'Fixture with failing addition test'
Push-Location $sample
try { & python -m unittest -v } finally { Pop-Location }
