import QtQuick
import Quickshell
import qs.Commons
import qs.Ui

// One button in the bar: opens Verso, or focuses it when it is already open.
BarWidget {
  id: root
  moduleName: "omarsimon.verso"

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  function open() {
    Quickshell.execDetached(["omarchy-launch-or-focus", "app.verso.Verso", "uwsm-app -- verso"])
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: ""
    slotSize: Style.bar.statusSlot
    fontSize: Style.font.caption
    tooltipText: "Verso"
    onPressed: root.open()
  }
}
