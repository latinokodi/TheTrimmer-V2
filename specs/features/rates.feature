Feature: Rates and frame grids
  A file's rate is what it claims; its grid is what it is. The two differ on real masters, and
  every one-frame error this product has had came from treating them as the same thing.

  Scenario: frames are counted on the grid the file is on
    Given a master that claims 30 fps and averages 156630000/5221099
    And its picture begins at 0.021 seconds
    When I convert its last frame to a time
    Then the time is one frame later than counting at 30 would give
    And converting that time back gives the same frame

  Scenario: a file that drifts from the rate it claims says so, with the number
    Given a master that claims 30 fps and averages 156630000/5221099
    When I plan a range on it
    Then the plan warns that its timestamps are away from that grid
    And the warning carries the measured drift

  Scenario: a constant rate file is its own grid, and is not warned about
    Given a master of 300 frames at exactly 25 fps
    When I plan a range on it
    Then the plan says nothing about its frame grid

  Scenario: a frame's time is measured from where the picture begins
    Given a 30 fps master whose picture begins at 0.100 seconds
    When I ask when frame 30 is shown
    Then the answer is 1.100 seconds

  Scenario: drop-frame is the reading the separator asks for
    Given a 29.97 fps master
    When I read "01:00:00:00"
    Then it names frame 108000
    When I read "01:00:00;00"
    Then it names frame 107892

  Scenario: a timecode that names a frame the rate cannot have is refused
    Given a 25 fps master
    When I read "00:00:00:25"
    Then the timecode is refused because the rate counts to 24

  Scenario: drop-frame is refused on a rate that has no drop-frame form
    Given a 25 fps master
    When I read "00:00:00;00"
    Then the timecode is refused because that rate has no drop-frame form

  Scenario: a timecode survives a round trip at every rate
    Given the rates 30000/1001,60000/1001,25,24000/1001,30
    When I write each of the frames 0, 1, 29, 1800 and 107892 and read it back
    Then every frame comes back unchanged
