Feature: Cutting a marked range out of a master
  A stream copy cannot begin on an arbitrary frame, so a range gets one of three methods. Which
  one it gets, and whether the operator is told before anything is written, is the whole of this
  feature.

  Background:
    Given a 30 fps master of 300 frames

  Scenario: a range that begins on a keyframe is copied whole
    Given its keyframes are at 4.0 seconds and 8.0 seconds
    When I mark frames 120 to 220
    Then the plan is a lossless copy
    And the plan re-encodes 0 frames

  Scenario: a range that begins between keyframes re-encodes only the head
    Given its keyframes are at 4.0 seconds and 8.0 seconds
    When I mark frames 100 to 200
    Then the plan is a head patch
    And the plan re-encodes 20 frames
    And the plan copies 80 frames untouched

  Scenario: a range with no keyframe in it is re-encoded whole, and the plan says so first
    Given its keyframes are at 10.0 seconds
    When I mark frames 100 to 200
    Then the plan is a full re-encode
    And the plan says there is no keyframe inside the segment

  Scenario: a body copy is aimed inside the GOP, because aiming at the keyframe takes the one before it
    Given a keyframe that opens at 100.0 seconds
    And the next keyframe at 108.334 seconds
    When a body of 7500 frames is copied from that keyframe
    Then the copy is aimed inside the GOP and not on its boundary
    And the length asked for is short by exactly the preroll the aim will include

  Scenario: the output is written on the source's own timescale
    Given its video timescale is 90000
    When I mark frames 100 to 200
    Then the head pass pins the timescale to 90000

  Scenario: a pass is bounded by time, never by a frame count
    Given its keyframes are at 4.0 seconds and 8.0 seconds
    When I mark frames 100 to 200
    Then no pass counts frames

  Scenario: the finished file is named for the range it holds
    When I name the output for frames 100 to 199 at 25 fps
    Then the name is "reel 00.00.04.00-00.00.07.24.mp4"
    And the name holds no character Windows refuses
