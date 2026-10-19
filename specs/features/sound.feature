Feature: The sound of a cut
  The picture is never re-encoded where a copy can carry it: a re-encoded head to the first
  keyframe, a copied body, a re-encoded tail. The sound has no keyframes to respect, so it is
  copied outright whenever the container can hold the master's own packets -- a second generation
  of audio is inaudible in isolation and permanent in practice.

  Scenario: a master the container can hold is copied, not encoded
    Given a master whose sound is aac and a segment whose container is mp4
    When the segment's sound is written
    Then the track is copied from the master's packets
    And the log says the sound was copied

  Scenario: a container that refuses the codec is encoded, and says so
    Given a master whose sound is pcm_s16le and a segment whose container is mp4
    When the segment's sound is written
    Then the track is encoded rather than copied
    And the log names the container as the reason
