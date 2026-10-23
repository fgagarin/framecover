"""Start screen and persistent footer widgets for the main window."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtCore import QPointF, QRectF, Qt, QUrl, Signal
from PySide6.QtGui import (
    QBrush,
    QColor,
    QDesktopServices,
    QIcon,
    QMouseEvent,
    QPainter,
    QPainterPath,
    QPen,
    QPixmap,
    QPolygonF,
)
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QMessageBox,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from framecover import __version__
from framecover.models import UpdateInfo, UpdateStatus

__all__ = [
    "AUTHOR_NAME",
    "PROJECT_URL",
    "SUPPORTED_FORMATS_TEXT",
    "AppFooter",
    "DropZone",
    "StartScreen",
]

AUTHOR_NAME = "Fedor Gagarin"
PROJECT_URL = "https://fgagarin.github.io/framecover/"
SUPPORTED_FORMATS_TEXT = "MP4, MOV, MKV, AVI, WEBM and other video formats"

_LINK = "#4f5fe6"
_DOT_LATEST = "#3b82f6"
_DOT_UPDATE = "#22c55e"
_DOT_IDLE = "#9ca3af"
_DOT_ERROR = "#f59e0b"

_ACCENT = QColor("#5b6cf0")
_ACCENT_DARK = QColor("#4c45d8")
_MUTED = "#6b7280"
_ICON_DPR = 2.0
_STEP_TILE_SIZE = 76
_STEP_ICON_SIZE = 48
_STEP_COLUMN_WIDTH = 190


def _render_icon(paint: Callable[[QPainter], None], size: int) -> QPixmap:
    pixmap = QPixmap(int(size * _ICON_DPR), int(size * _ICON_DPR))
    pixmap.setDevicePixelRatio(_ICON_DPR)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    painter.scale(size / 64, size / 64)
    paint(painter)
    painter.end()
    return pixmap


def _stroke(width: float = 4.0, color: QColor = _ACCENT) -> QPen:
    pen = QPen(color, width)
    pen.setCapStyle(Qt.PenCapStyle.RoundCap)
    pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
    return pen


def _translucent(alpha: int) -> QColor:
    color = QColor(_ACCENT)
    color.setAlpha(alpha)
    return color


def _paint_mountains(painter: QPainter, rect: QRectF) -> None:
    left, top, width, height = rect.x(), rect.y(), rect.width(), rect.height()
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(_ACCENT)
    painter.drawEllipse(
        QPointF(left + width * 0.3, top + height * 0.3), width * 0.1, width * 0.1
    )
    painter.drawPolygon(
        QPolygonF(
            [
                QPointF(left, top + height),
                QPointF(left + width * 0.38, top + height * 0.45),
                QPointF(left + width * 0.58, top + height * 0.7),
                QPointF(left + width * 0.72, top + height * 0.55),
                QPointF(left + width, top + height),
            ]
        )
    )


def _paint_open_file(painter: QPainter) -> None:
    path = QPainterPath()
    path.moveTo(14, 6)
    path.lineTo(40, 6)
    path.lineTo(52, 18)
    path.lineTo(52, 58)
    path.lineTo(14, 58)
    path.closeSubpath()
    painter.setPen(_stroke())
    painter.setBrush(_translucent(25))
    painter.drawPath(path)
    painter.setBrush(Qt.BrushStyle.NoBrush)
    painter.drawPolyline(QPolygonF([QPointF(40, 6), QPointF(40, 18), QPointF(52, 18)]))
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(_ACCENT_DARK)
    painter.drawPolygon(
        QPolygonF([QPointF(26, 27), QPointF(26, 47), QPointF(42, 37)])
    )


def _paint_choose_frame(painter: QPainter) -> None:
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(_translucent(45))
    painter.drawRoundedRect(QRectF(2, 18, 60, 28), 3, 3)
    painter.setBrush(QColor(255, 255, 255, 170))
    for x in (5, 55):
        for y in (22, 30, 38):
            painter.drawRect(QRectF(x, y, 4, 4))
    frame = QRectF(14, 16, 36, 32)
    painter.setPen(_stroke(3.5))
    painter.setBrush(QColor(255, 255, 255, 200))
    painter.drawRoundedRect(frame, 3, 3)
    _paint_mountains(painter, frame.adjusted(5, 6, -5, -5))
    painter.setPen(_stroke(4.0, _ACCENT_DARK))
    painter.drawLine(QPointF(32, 4), QPointF(32, 60))


def _paint_save_preview(painter: QPainter) -> None:
    frame = QRectF(8, 12, 48, 40)
    painter.setPen(_stroke())
    painter.setBrush(_translucent(25))
    painter.drawRoundedRect(frame, 6, 6)
    _paint_mountains(painter, frame.adjusted(5, 6, -5, -5))


def _paint_folder(painter: QPainter) -> None:
    path = QPainterPath()
    path.moveTo(8, 18)
    path.quadTo(8, 13, 13, 13)
    path.lineTo(24, 13)
    path.lineTo(30, 19)
    path.lineTo(51, 19)
    path.quadTo(56, 19, 56, 24)
    path.lineTo(56, 47)
    path.quadTo(56, 52, 51, 52)
    path.lineTo(13, 52)
    path.quadTo(8, 52, 8, 47)
    path.closeSubpath()
    painter.setPen(_stroke(3.5))
    painter.setBrush(QBrush(Qt.BrushStyle.NoBrush))
    painter.drawPath(path)
    painter.drawLine(QPointF(8, 25), QPointF(56, 25))


class DropZone(QFrame):
    """Dashed, clickable area that invites the user to open or drop a video."""

    clicked = Signal()

    def __init__(self, parent: QWidget | None = None) -> None:
        """Create the drop zone.

        Args:
            parent: Optional parent widget.
        """
        super().__init__(parent)
        self.setObjectName("dropZone")
        self.setAttribute(Qt.WidgetAttribute.WA_Hover)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.setAccessibleName("Open video file")
        self.setMinimumHeight(180)
        self.setStyleSheet(
            "QFrame#dropZone {"
            "  border: 2px dashed rgba(91, 108, 240, 150);"
            "  border-radius: 12px;"
            "  background: rgba(91, 108, 240, 14);"
            "}"
            "QFrame#dropZone:hover {"
            "  border-color: rgba(91, 108, 240, 230);"
            "  background: rgba(91, 108, 240, 28);"
            "}"
        )

        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 20)
        layout.setSpacing(4)
        layout.addStretch(1)

        icon = QLabel()
        icon.setPixmap(_render_icon(_paint_folder, 56))
        icon.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.addWidget(icon)
        layout.addSpacing(6)

        self.title_label = QLabel()
        self.title_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.title_label.setStyleSheet("font-size: 18px; font-weight: 600;")
        layout.addWidget(self.title_label)

        self.hint_label = QLabel()
        self.hint_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.hint_label.setWordWrap(True)
        self.hint_label.setTextFormat(Qt.TextFormat.PlainText)
        self.hint_label.setStyleSheet(f"color: {_MUTED}; font-size: 15px;")
        layout.addWidget(self.hint_label)
        layout.addSpacing(14)

        self.formats_label = QLabel(SUPPORTED_FORMATS_TEXT)
        self.formats_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.formats_label.setStyleSheet(f"color: {_MUTED}; font-size: 12px;")
        layout.addWidget(self.formats_label)
        layout.addStretch(1)

        self.reset()

    def reset(self) -> None:
        """Show the default call-to-action text."""
        self.title_label.setText("Click to open a video file")
        self.hint_label.setText("or drag and drop a file here")

    def set_loading(self, path: str) -> None:
        """Show that *path* is being loaded.

        Args:
            path: Path of the video being opened.
        """
        self.title_label.setText("Loading video…")
        self.hint_label.setText(path)

    def mouseReleaseEvent(self, event: QMouseEvent) -> None:  # noqa: N802
        """Emit :attr:`clicked` on a left-button release inside the zone."""
        super().mouseReleaseEvent(event)
        if event.button() == Qt.MouseButton.LeftButton and self.rect().contains(
            event.position().toPoint()
        ):
            self.clicked.emit()


class StartScreen(QWidget):
    """Welcome screen: headline, three-step instructions, and a drop zone."""

    def __init__(self, parent: QWidget | None = None) -> None:
        """Create the start screen.

        Args:
            parent: Optional parent widget.
        """
        super().__init__(parent)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(40, 8, 40, 8)
        layout.setSpacing(0)
        layout.addStretch(1)

        self.title_label = QLabel("Create a preview from any video")
        self.title_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.title_label.setWordWrap(True)
        self.title_label.setStyleSheet("font-size: 30px; font-weight: 700;")
        layout.addWidget(self.title_label)
        layout.addSpacing(6)

        self.subtitle_label = QLabel(
            "Open a video, choose a frame, and save it as an image."
        )
        self.subtitle_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.subtitle_label.setWordWrap(True)
        self.subtitle_label.setStyleSheet(f"color: {_MUTED}; font-size: 15px;")
        layout.addWidget(self.subtitle_label)
        layout.addSpacing(28)

        steps = QHBoxLayout()
        steps.setSpacing(0)
        steps.addStretch(1)
        self.step_labels: list[QLabel] = []
        step_specs = [
            ("1. Open a file", _paint_open_file),
            ("2. Choose a frame", _paint_choose_frame),
            ("3. Save the preview", _paint_save_preview),
        ]
        for index, (text, paint) in enumerate(step_specs):
            if index:
                arrow = QLabel("→")
                arrow.setAlignment(Qt.AlignmentFlag.AlignHCenter)
                arrow.setStyleSheet(
                    f"color: {_MUTED}; font-size: 24px; padding-top: 20px;"
                )
                steps.addWidget(arrow, 0, Qt.AlignmentFlag.AlignTop)
            steps.addWidget(self._make_step(text, paint))
        steps.addStretch(1)
        layout.addLayout(steps)
        layout.addSpacing(28)

        self.drop_zone = DropZone()
        layout.addWidget(self.drop_zone, 2)
        layout.addStretch(1)

    def _make_step(self, text: str, paint: Callable[[QPainter], None]) -> QWidget:
        column = QWidget()
        column.setFixedWidth(_STEP_COLUMN_WIDTH)
        column_layout = QVBoxLayout(column)
        column_layout.setContentsMargins(0, 0, 0, 0)
        column_layout.setSpacing(10)

        tile = QLabel()
        tile.setFixedSize(_STEP_TILE_SIZE, _STEP_TILE_SIZE)
        tile.setAlignment(Qt.AlignmentFlag.AlignCenter)
        tile.setPixmap(_render_icon(paint, _STEP_ICON_SIZE))
        tile.setStyleSheet(
            "background: rgba(128, 128, 128, 26); border-radius: 16px;"
        )
        column_layout.addWidget(tile, 0, Qt.AlignmentFlag.AlignHCenter)

        label = QLabel(text)
        label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        label.setStyleSheet("font-size: 15px;")
        column_layout.addWidget(label)
        self.step_labels.append(label)
        return column


class AppFooter(QWidget):
    """Bottom bar with version, author, project link, and update status.

    Signals:
        check_requested(): Emitted when the user clicks the update indicator
            and no newer release is available.
    """

    check_requested = Signal()

    def __init__(self, parent: QWidget | None = None) -> None:
        """Create the footer.

        Args:
            parent: Optional parent widget.
        """
        super().__init__(parent)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(4, 0, 0, 0)
        layout.setSpacing(12)

        self.version_label = QLabel(f"FrameCover {__version__}")
        self.author_label = QLabel(f"by {AUTHOR_NAME}")
        self.link_label = QLabel(
            f'<a href="{PROJECT_URL}" style="color: #4f5fe6;">Project page</a> ↗'
        )
        self.link_label.setTextFormat(Qt.TextFormat.RichText)
        self.link_label.setTextInteractionFlags(
            Qt.TextInteractionFlag.TextBrowserInteraction
        )
        self.link_label.setOpenExternalLinks(True)
        self.link_label.setToolTip(PROJECT_URL)

        info_widgets = [self.version_label, self.author_label, self.link_label]
        for index, widget in enumerate(info_widgets):
            if index:
                separator = QFrame()
                separator.setFrameShape(QFrame.Shape.VLine)
                separator.setFixedHeight(14)
                separator.setStyleSheet("color: rgba(128, 128, 128, 90);")
                layout.addWidget(separator)
            widget.setStyleSheet(f"color: {_MUTED}; font-size: 12px;")
            layout.addWidget(widget)
        layout.addStretch(1)

        self._release_url: str | None = None
        self.status_indicator = QPushButton()
        self.status_indicator.setFlat(True)
        self.status_indicator.setCursor(Qt.CursorShape.PointingHandCursor)
        self.status_indicator.setStyleSheet(
            "QPushButton {"
            "  background: transparent; border: none;"
            f"  color: {_MUTED}; padding: 2px; font-size: 12px;"
            "}"
            f"QPushButton:hover {{ color: {_LINK}; text-decoration: underline; }}"
            f"QPushButton:focus {{ color: {_LINK}; text-decoration: underline; }}"
            "QPushButton:disabled { color: rgba(128, 128, 128, 140); }"
        )
        self.status_indicator.clicked.connect(self._activate_status)
        layout.addWidget(self.status_indicator)

        self._set_status(_DOT_IDLE, "Updates not checked yet")

    def set_checking(self) -> None:
        """Show that an update check is in progress."""
        self._set_status(_DOT_IDLE, "Checking for updates…")
        self.status_indicator.setEnabled(False)
        self.status_indicator.setToolTip("Checking GitHub for updates. Please wait.")

    def set_update_info(self, info: UpdateInfo) -> None:
        """Show the result of a successful update check.

        Clicking an available update opens its GitHub release page. Otherwise
        clicking the indicator runs another check.

        Args:
            info: Comparison between the running and the latest version.
        """
        if info.status is UpdateStatus.UPDATE_AVAILABLE:
            self._set_status(
                _DOT_UPDATE,
                f"Version {info.latest_version} is available",
                f"You have {info.current_version}. Open the release page on "
                "GitHub to read what changed and download the new version, "
                "then install it over the current one.",
            )
            self._release_url = info.release_url
            self.status_indicator.setToolTip(
                f"You have {info.current_version}. Click to open this release "
                "on GitHub and download the new version."
            )
        elif info.status is UpdateStatus.NEWER_THAN_RELEASE:
            self._set_status(
                _DOT_LATEST,
                "Test build · newer than the latest release",
                f"Latest published release: {info.latest_version}.",
            )
        else:
            self._set_status(_DOT_LATEST, "You have the latest version")

    def set_check_failed(self, message: str) -> None:
        """Show that the update check failed and how to retry.

        Args:
            message: User-facing explanation of the failure and next steps.
        """
        self._set_status(_DOT_ERROR, "Couldn't check for updates", message)

    def _set_status(self, color: str, text: str, tooltip: str = "") -> None:
        self._release_url = None
        dot = QPixmap(12, 12)
        dot.fill(Qt.GlobalColor.transparent)
        painter = QPainter(dot)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(QColor(color))
        painter.drawEllipse(QRectF(1, 1, 10, 10))
        painter.end()
        self.status_indicator.setIcon(QIcon(dot))
        self.status_indicator.setText(text)
        self.status_indicator.setToolTip(
            f"{tooltip}\nClick this indicator to check for updates.".strip()
        )
        self.status_indicator.setEnabled(True)

    def _activate_status(self) -> None:
        if self._release_url is None:
            self.check_requested.emit()
        elif not QDesktopServices.openUrl(QUrl(self._release_url)):
            QMessageBox.warning(
                self,
                "Couldn't open the release page",
                "Open this address in your browser to download the new version:\n"
                f"{self._release_url}",
            )
