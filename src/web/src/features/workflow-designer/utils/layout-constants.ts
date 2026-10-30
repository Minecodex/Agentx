import { STUDIO_BOUNDARY_WIDTH, STUDIO_BOUNDARY_HEIGHT } from '../nodes/node-appearance'

export const LAYOUT_LAYER_GAP = 96
export const LAYOUT_NODE_GAP = 64
export const LOOP_CONTAINER_MIN_WIDTH = 520
export const LOOP_CONTAINER_MIN_HEIGHT = 280
export const LOOP_CONTAINER_DEFAULT_WIDTH = 640
export const LOOP_CONTAINER_DEFAULT_HEIGHT = 360
export const LOOP_CHIP_POSITION = { x: 24, y: 84 }
export const LOOP_END_CHIP_WIDTH = STUDIO_BOUNDARY_WIDTH
export const LOOP_END_CHIP_HEIGHT = STUDIO_BOUNDARY_HEIGHT
const boundaryGutter = LOOP_CHIP_POSITION.x + STUDIO_BOUNDARY_WIDTH + LAYOUT_LAYER_GAP
export const LOOP_CONTAINER_PADDING = { top: LOOP_CHIP_POSITION.y, right: boundaryGutter, bottom: 96, left: boundaryGutter }
export const LOOP_FIRST_CHILD_POSITION = { x: boundaryGutter, y: LOOP_CHIP_POSITION.y }
export const loopEndChipPosition = (width: number) => ({ x: width - LOOP_END_CHIP_WIDTH - 24, y: LOOP_CHIP_POSITION.y })
