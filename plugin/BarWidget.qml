import QtQuick
import Quickshell
import qs.Commons
import qs.Ui

// One button in the bar: opens Jot, or focuses it when it is already open.
BarWidget {
  id: root
  moduleName: "omarsimon.jot"

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  function open() {
    Quickshell.execDetached(["omarchy-launch-or-focus", "app.jot.Jot", "uwsm-app -- jot"])
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: ""
    slotSize: Style.bar.statusSlot
    fontSize: Style.font.caption
    tooltipText: "Jot"
    onPressed: root.open()
  }
}
