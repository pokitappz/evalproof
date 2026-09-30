"""The wheel contains a native executable, but no CPython extension ABI."""
import os
from setuptools import setup
from setuptools.command.bdist_wheel import bdist_wheel


class BinaryWheel(bdist_wheel):
    def finalize_options(self):
        super().finalize_options()
        self.root_is_pure = False
        if os.environ.get("EVALPROOF_WHEEL_PLATFORM"):
            self.plat_name = os.environ["EVALPROOF_WHEEL_PLATFORM"]

    def get_tag(self):
        return "py3", "none", self.plat_name.replace("-", "_").replace(".", "_")


setup(cmdclass={"bdist_wheel": BinaryWheel})
