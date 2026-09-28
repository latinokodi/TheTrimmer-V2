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

  Scenario: a frame's stated time comes from a window wide enough to contain it
    Given a 30 fps master whose frames are 1/30 of a second apart
    When I ask the container for the stated time of a frame deep in the file
    Then the read opens before that frame and closes after it
    And the read does not begin at the start of the file

  Scenario: the container is never asked to guess which frame was meant
    Given a 30 fps master whose frames are 1/30 of a second apart
    When the container answers with frames that are not the one asked about
    Then no time is returned for that frame
    And the plan falls back to the time computed from the grid

  Scenario: the finished file is named for the range it holds
    When I name the output for frames 100 to 199 at 25 fps
    Then the name is "reel 00.00.04.00-00.00.07.24.mp4"
    And the name holds no character Windows refuses

  Scenario: a segment is named after what the operator calls it
    When I name the segment "Interview wide"
    Then the segment is called "Interview wide.mp4"
    And the segment sits beside its source
    And the transcript is called "Interview wide.srt"
    And the transcript sits beside the segment

  Scenario: a named segment can arrive in a folder of its own
    When I name the segment "Interview wide" and ask for a folder
    Then the segment and its transcript are inside a folder named "Interview wide"
    And that folder sits beside the source

  Scenario: a name Windows would refuse is refused, and the reason says which
    When I name the segment "Take 1:2"
    Then the name is refused
    And the reason names the character that was objected to

  Scenario: a name the filesystem accepts is not refused for being unusual
    When I name the segment "Émilie — finale 2.1"
    Then the segment is called "Émilie — finale 2.1.mp4"

  Scenario: a name is resolved before a range has been marked
    Given no range is marked
    When I ask where the segment named "Interview wide" will be written
    Then the answer is a path called "Interview wide.mp4"
    And the source was not read

  Scenario: a refusal says which field it is about
    When I ask where the segment named "Take 1:2" will be written
    Then the refusal is tagged as being about the name
