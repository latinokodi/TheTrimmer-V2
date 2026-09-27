Feature: Cutting a marked range out of a master
  A stream copy cannot begin on an arbitrary frame, and it cannot stop on one either. So a range
  is built from three pieces -- the run to the keyframe that opens the body, the original packets,
  and the run from the last keyframe before the out point. Which one it gets, and whether the
  operator is told before anything is written, is the whole of this feature.

  Background:
    Given a 25 fps master of 400 frames

  Scenario: a range that begins on a keyframe re-encodes only its far end
    Given its keyframes are at 4.0 seconds, 5.0 seconds and 10.0 seconds
    When I mark frames 100 to 300
    Then the plan is a lossless copy
    And the plan re-encodes 0 frames at the head and 50 at the tail
    And the plan copies 150 frames untouched

  Scenario: a range that begins between keyframes re-encodes both ends
    Given its keyframes are at 4.0 seconds, 8.0 seconds and 12.0 seconds
    When I mark frames 150 to 350
    Then the plan is a head patch
    And the plan re-encodes 50 frames at the head and 50 at the tail
    And the plan copies 100 frames untouched

  Scenario: a range with no keyframe boundary after its in point is re-encoded whole
    Given its keyframes are at 4.0 seconds and 12.0 seconds
    When I mark frames 150 to 250
    Then the plan is a full re-encode
    And the plan copies 0 frames untouched

  Scenario: a range with no keyframe in it is re-encoded whole, and the plan says so first
    Given its keyframes are at 10.0 seconds
    When I mark frames 100 to 200
    Then the plan is a full re-encode
    And the plan says there is no keyframe inside the segment

  Scenario: a body copy takes exactly the frames the plan names, by index
    Given a keyframe that opens at 100.0 seconds
    When a body of 7500 frames is copied from that keyframe
    Then the copy takes exactly the frames the plan names, by index
    And the length is not asked for as a span of time

  Scenario: the output is written on the source's own timescale
    Given its video timescale is 90000
    When I mark frames 100 to 200
    Then the head pass pins the timescale to 90000

  Scenario: a pass that selects original packets is bounded by time, never by a frame count
    Given its keyframes are at 4.0 seconds and 8.0 seconds
    When I mark frames 100 to 200
    Then no pass that copies packets counts frames

  Scenario: a re-encoded piece pins its own length, because nothing is dropped by counting
    Given its keyframes are at 4.0 seconds and 8.0 seconds
    When I mark frames 100 to 200
    Then both re-encoded ends are bounded by the frame count the plan names

  Scenario: the ends are seeked to the time the container states, not to a computed one
    Given a 30 fps master whose picture begins at 0.0 seconds
    When the plan carries the container's own time for the in point
    Then both re-encoded ends are seeked to the time the container states

  Scenario: the finished file is named for the range it holds
    When I name the output for frames 100 to 199 at 25 fps
    Then the name is "reel 00.00.04.00-00.00.07.24.mp4"
    And the name holds no character Windows refuses
