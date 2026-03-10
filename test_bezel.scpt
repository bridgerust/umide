tell application "System Events"
    tell process "Simulator"
        if exists menu item "Show Device Bezels" of menu "Window" of menu bar 1 then
            set bezelItem to menu item "Show Device Bezels" of menu "Window" of menu bar 1
            if value of attribute "AXMenuItemMarkChar" of bezelItem is not missing value then
                click bezelItem
                return "Bezels disabled"
            else
                return "Bezels already disabled"
            end if
        end if
    end tell
end tell
