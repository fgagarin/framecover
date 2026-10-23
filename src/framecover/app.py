"""Application entry point for FrameCover."""

from __future__ import annotations

import ctypes
import sys
from pathlib import Path

from PySide6.QtGui import QIcon


def _resource_path(name: str) -> Path:
    bundle_dir = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parents[2]))
    return bundle_dir / name


def _set_windows_app_user_model_id() -> None:
    if sys.platform == "win32":
        ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(
            ctypes.c_wchar_p("FrameCover.Application")
        )


def main() -> None:
    """Launch the FrameCover GUI application."""
    from PySide6.QtWidgets import QApplication

    from framecover.core.frame_extractor import PyAVFrameExtractor
    from framecover.core.thumbnail_writer import FormatDispatchThumbnailWriter
    from framecover.core.video_converter import FFmpegVideoConverter
    from framecover.core.video_loader import PyAVVideoLoader
    from framecover.platform import get_cache_invalidator
    from framecover.ui.error_dialog import install_exception_handler
    from framecover.ui.main_window import MainWindow

    _set_windows_app_user_model_id()
    app = QApplication(sys.argv)
    icon = QIcon(str(_resource_path("icon.png")))
    app.setWindowIcon(icon)
    exception_handler = install_exception_handler(app.activeWindow)

    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    writer = FormatDispatchThumbnailWriter()
    invalidator = get_cache_invalidator()

    window = MainWindow(
        loader, extractor, writer, invalidator, converter=FFmpegVideoConverter()
    )
    window.setWindowIcon(icon)
    window.show()

    exit_code = app.exec()
    exception_handler.uninstall()
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
