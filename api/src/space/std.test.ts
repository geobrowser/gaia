import { describe, expect, it } from "vitest"
import { EditProposal } from "@graphprotocol/grc-20/proto"
import { Id, Graph } from "@graphprotocol/grc-20"
import { generateEditFormData } from "./std"

describe("generateEditFormData", () => {
	it("should create FormData with encoded EditProposal content", () => {
		// Create an EditProposal and encode it
		const editProposal = {
			name: "Test Space",
			author: "0x1234567890123456789012345678901234567890" as `0x${string}`,
			ops: []
		}
		
		const encodedContent = EditProposal.encode(editProposal)
		
		// Generate FormData
		const formData = generateEditFormData(encodedContent)
		
		// Verify FormData structure
		expect(formData).toBeInstanceOf(FormData)
		expect(formData.has("file")).toBe(true)
		
		const file = formData.get("file")
		expect(file).toBeInstanceOf(Blob)
		
		if (file instanceof Blob) {
			expect(file.type).toBe("application/octet-stream")
			// The size should be greater than 0
			expect(file.size).toBeGreaterThan(0)
		}
	})
	
	it("should handle Uint8Array with ArrayBufferLike from protobuf encoding", () => {
		// Test with a more complex EditProposal with operations
		const editProposal = {
			name: "Production Space",
			author: "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd" as `0x${string}`,
			ops: [
				{
					type: "UPDATE_ENTITY" as const,
					entity: {
						id: Id.Id("550e8400-e29b-41d4-a716-446655440000"),
						name: "TestEntity",
						values: []
					}
				}
			]
		}
		
		const encodedContent = EditProposal.encode(editProposal)
		
		// This should not throw despite the Uint8Array<ArrayBufferLike> type
		const formData = generateEditFormData(encodedContent)
		
		expect(formData).toBeInstanceOf(FormData)
		const file = formData.get("file")
		expect(file).toBeInstanceOf(Blob)
	})
	
	it("should preserve the encoded content size", async () => {
		const editProposal = {
			name: "Integrity Test Space",
			author: "0x5555555555555555555555555555555555555555" as `0x${string}`,
			ops: []
		}
		
		const encodedContent = EditProposal.encode(editProposal)
		const originalLength = encodedContent.length
		
		const formData = generateEditFormData(encodedContent)
		const file = formData.get("file") as Blob
		
		// Read the blob content and verify size matches
		expect(file.size).toBe(originalLength)
		
		// Verify the content can be read back
		const arrayBuffer = await file.arrayBuffer()
		const readContent = new Uint8Array(arrayBuffer)
		
		// Verify the byte content matches exactly
		expect(readContent.length).toBe(encodedContent.length)
		for (let i = 0; i < encodedContent.length; i++) {
			expect(readContent[i]).toBe(encodedContent[i])
		}
	})
})